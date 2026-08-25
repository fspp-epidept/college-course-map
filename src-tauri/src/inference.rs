//! Rust ONNX inference, mirror of `scripts/models/_lib/inference.py`. Parity
//! against the Python pipeline is asserted by `examples/check_parity.rs` over
//! the fixture in `scripts/models/output/parity/per_input.json`.
//!
//! One [`LoadedModel`] per digit level. Models hold their own session +
//! tokenizer + id->label table; they are not Send-shared at this stage because
//! the spike run pipeline drives them synchronously from the IPC thread.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use ort::{
    ep::{self, ExecutionProvider as _},
    inputs,
    session::{Session, builder::GraphOptimizationLevel},
    value::TensorRef,
};
use serde::Deserialize;

use crate::runtime::EpKind;
use tokenizers::{
    PaddingDirection, PaddingParams, PaddingStrategy, Tokenizer, TruncationDirection,
    TruncationParams,
};

/// Matches `max_length=512` in `scripts/models/_lib/inference.py::predict_batch`.
const MAX_SEQ_LEN: usize = 512;

/// Per-EP inference batch size (EPI-82): how many inputs go through one
/// `session.run` call. Besides the ONNX call, this is also the run worker's
/// progress/flush/cancel granularity.
///
/// Measured 2026-07-28/29 on the validation panel (RTX 4070 SUPER, ONNX
/// Runtime 1.24.2 cuda13 pack, two-digit model, `task check:throughput`).
/// These constants assume the run worker's length-bucketing (EPI-82: inputs
/// sorted by length within a super-chunk, so `BatchLongest` pads almost
/// nothing): bucketed batch 128 is the optimum on *both* CUDA (4,514 unique
/// rows/s; 64 ≈ −3%, 256 ≈ −18%) and CPU (166 rows/s; +16% over the old
/// unbucketed 64). Unbucketed, larger batches lose badly to padding waste —
/// don't raise this without re-measuring via `task check:throughput
/// --bucket --batch N`. The EP parameter is the tuning seam; DirectML/CoreML
/// are unmeasured and inherit the measured optimum.
#[must_use]
pub fn batch_size(_ep: EpKind) -> usize {
    128
}

/// A model loaded into ONNX Runtime with its tokenizer + class label table.
pub struct LoadedModel {
    pub digit_level: u8,
    /// The highest-priority execution provider that registered successfully
    /// for this session (EPI-73); `Cpu` when none did. Recorded on runs rows
    /// and surfaced in Settings.
    pub resolved_ep: EpKind,
    /// The session needs `&mut self` to run; wrap so we can hold it behind an
    /// `Arc` shared from the inference registry.
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    /// Index → CCM code string, e.g. `id2label[14] == "27"` for the 2-digit
    /// model.
    id2label: Vec<String>,
}

impl std::fmt::Debug for LoadedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedModel")
            .field("digit_level", &self.digit_level)
            .field("num_labels", &self.id2label.len())
            .finish_non_exhaustive()
    }
}

/// One ranked candidate within a [`Classification`]: class index, its raw
/// `id2label` string (float-mangled — normalize with [`normalize_ccm_code`]
/// before persisting), and its softmax probability.
#[derive(Debug, Clone)]
pub struct TopCandidate {
    pub index: usize,
    pub label: String,
    pub probability: f32,
}

/// One classification: argmax label, its index, the top-5 candidates (sorted
/// descending by logit), the raw logit value at argmax, and the softmax
/// probability at argmax (the confidence; see `docs/model-confidence.md`).
#[derive(Debug, Clone)]
pub struct Classification {
    pub label: String,
    pub label_index: usize,
    /// Rank 1 is always the argmax — same label and probability as the
    /// top-level fields (EPI-98 persists ranks 2–5; rank 1 lives in the
    /// existing `classification`/`probability` cache columns).
    pub top5: [TopCandidate; 5],
    pub logit_argmax: f32,
    pub probability: f32,
}

/// Execution providers whose registration failed earlier in this process
/// (EPI-104). Retrying a failed provider on a later session is the one lethal
/// path in `ort` 2.0.0-rc.12: `ep::DirectML::register` caches the DML API
/// table in a `std`-flavoured `OnceLock` whose `get_or_try_init` marks the
/// cell completed even when the init closure returns `Err`, so the second
/// attempt reads zeroed memory as the API table and calls through a null
/// function pointer (`0xc0000005` at offset 0 on Windows, `SIGSEGV` on
/// Linux). Success-then-reuse is safe — a valid table is cached — so only
/// failures are memoised, for the process lifetime. Per-session registration
/// itself is required ONNX Runtime semantics (EPs bind to `SessionOptions`);
/// this hoists the *decision*, not the registration.
struct FailedEps(Mutex<Vec<EpKind>>);

impl FailedEps {
    const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn contains(&self, ep: EpKind) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&ep)
    }

    fn record(&self, ep: EpKind) {
        let mut failed = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !failed.contains(&ep) {
            failed.push(ep);
        }
    }
}

static FAILED_EPS: FailedEps = FailedEps::new();

/// Register execution providers on a session builder in priority order
/// (EPI-73). Returns the first EP that registered successfully — the one ONNX
/// Runtime will prefer when assigning graph nodes — or `Cpu` when none did.
///
/// A failed registration is the designed fallback path (CUDA/cuDNN/TensorRT
/// libs not installed on this machine, driver too old), so it logs and
/// continues rather than erroring — and is remembered in [`FAILED_EPS`] so no
/// later session retries it. Callers pass a list already filtered against the
/// loaded pack's claimed providers (`RuntimeState::registrable`); the memo is
/// the second guard for a pack whose metadata claims a provider its dylib
/// doesn't carry. `Cpu` in the list is the fallback boundary: entries after it
/// are below the implicit CPU EP by definition and never register.
fn register_eps(
    builder: &mut ort::session::builder::SessionBuilder,
    eps: &[EpKind],
) -> Option<EpKind> {
    let mut resolved = None;
    for &ep in eps {
        if ep == EpKind::Cpu {
            break;
        }
        if FAILED_EPS.contains(ep) {
            eprintln!(
                "execution provider {}: skipped (failed earlier in this process)",
                ep.as_str()
            );
            continue;
        }
        let result = match ep {
            EpKind::Cpu => break,
            EpKind::TensorRt => ep::TensorRT::default().register(builder),
            EpKind::Cuda => ep::CUDA::default().register(builder),
            EpKind::DirectMl => ep::DirectML::default().register(builder),
            EpKind::CoreMl => ep::CoreML::default().register(builder),
        };
        match result {
            Ok(()) => {
                if resolved.is_none() {
                    resolved = Some(ep);
                }
            }
            Err(e) => {
                FAILED_EPS.record(ep);
                eprintln!("execution provider {}: not used: {e}", ep.as_str());
            }
        }
    }
    resolved
}

/// Build a [`LoadedModel`] from a directory containing `model.onnx`,
/// `tokenizer.json`, and `config.json`. `eps` is the user's execution-provider
/// priority list (settings), applied to the session before commit.
/// `max_cpu_threads` caps ORT's intra-op pool (EPI-83): 0, or any value
/// outside `1..=cores`, means auto (ORT's default — all physical cores).
pub fn load_model(
    model_dir: &Path,
    digit_level: u8,
    eps: &[EpKind],
    max_cpu_threads: u32,
) -> anyhow::Result<LoadedModel> {
    // Parse config.json first: it carries id2label AND pad_token_id, which
    // the padding setup below needs. The pad token is family-specific
    // (RoBERTa: 1/"<pad>", ModernBERT: 50283/"[PAD]") — deriving it from the
    // model's own config instead of hardcoding keeps the loader correct for
    // whichever family the manifest pins.
    let config = load_hf_config(&model_dir.join("config.json"))?;
    let id2label = id2label_table(&config)?;

    let mut tokenizer = Tokenizer::from_file(model_dir.join("tokenizer.json"))
        .map_err(|e| anyhow::anyhow!("load tokenizer: {e}"))?;
    // Match the Python pipeline: `truncation=True, max_length=512`.
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: MAX_SEQ_LEN,
            direction: TruncationDirection::Right,
            ..Default::default()
        }))
        .map_err(|e| anyhow::anyhow!("set truncation: {e}"))?;
    let pad_token = tokenizer.id_to_token(config.pad_token_id).ok_or_else(|| {
        anyhow::anyhow!(
            "config.json pad_token_id {} not present in tokenizer vocab",
            config.pad_token_id
        )
    })?;
    // BatchLongest = pad to the longest sequence in each batch. For a single
    // input (the `classify` path and the parity fixture), the "batch" has one
    // entry so this is a no-op — outputs stay byte-identical to the un-padded
    // path. For real batches (the run worker), this gives encode_batch uniform
    // [n, max_len] shapes ready to flatten into a tensor.
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        direction: PaddingDirection::Right,
        pad_to_multiple_of: None,
        pad_id: config.pad_token_id,
        pad_type_id: 0,
        pad_token,
    }));

    // ort's `Error` carries a builder phantom that's not `Send + Sync`, so we
    // can't `?` it into `anyhow::Error`; stringify at the boundary.
    let mut builder = Session::builder()
        .map_err(|e| anyhow::anyhow!("session builder: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow::anyhow!("set opt level: {e}"))?;
    // Clamp semantics (EPI-83): only a value in 1..=cores overrides ORT's
    // default pool size; anything else (0 = auto, or out of range) leaves the
    // default, which already means "all physical cores".
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    if (1..=cores).contains(&(max_cpu_threads as usize)) {
        builder = builder
            .with_intra_threads(max_cpu_threads as usize)
            .map_err(|e| anyhow::anyhow!("set intra threads: {e}"))?;
    }
    let resolved_ep = register_eps(&mut builder, eps).unwrap_or(EpKind::Cpu);
    let session = builder
        .commit_from_file(model_dir.join("model.onnx"))
        .map_err(|e| anyhow::anyhow!("commit_from_file: {e}"))?;

    Ok(LoadedModel {
        digit_level,
        resolved_ep,
        session: Mutex::new(session),
        tokenizer,
        id2label,
    })
}

/// Run one input through the model. Thin wrapper over [`classify_batch`] so
/// the parity fixture exercises the same code path as the batched run worker.
pub fn classify(model: &LoadedModel, input: &str) -> anyhow::Result<Classification> {
    classify_batch(model, &[input])?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("classify_batch returned no rows for single input"))
}

/// Batched inference. Tokenizes the whole batch with `encode_batch` so the
/// tokenizer's `BatchLongest` padding produces uniform `[n, max_len]` shapes,
/// then runs a single ONNX session call. Empty input returns an empty Vec.
pub fn classify_batch(model: &LoadedModel, inputs: &[&str]) -> anyhow::Result<Vec<Classification>> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }

    let encodings = model
        .tokenizer
        .encode_batch(inputs.to_vec(), true)
        .map_err(|e| anyhow::anyhow!("encode_batch: {e}"))?;
    let n = encodings.len();
    let max_len = encodings
        .iter()
        .map(|e| e.get_ids().len())
        .max()
        .unwrap_or(0);
    if max_len == 0 {
        anyhow::bail!("encode_batch produced empty encodings");
    }

    let mut ids: Vec<i64> = Vec::with_capacity(n * max_len);
    let mut mask: Vec<i64> = Vec::with_capacity(n * max_len);
    for enc in &encodings {
        let row_ids = enc.get_ids();
        let row_mask = enc.get_attention_mask();
        // BatchLongest pads every row to max_len, so this is a length check we
        // expect to always pass — but if the tokenizer config ever changes,
        // failing here is far better than feeding ONNX a ragged tensor.
        if row_ids.len() != max_len || row_mask.len() != max_len {
            anyhow::bail!(
                "ragged encoding: ids={}, mask={}, expected {max_len}",
                row_ids.len(),
                row_mask.len()
            );
        }
        ids.extend(row_ids.iter().map(|&i| i64::from(i)));
        mask.extend(row_mask.iter().map(|&m| i64::from(m)));
    }

    let n_i64 = i64::try_from(n).map_err(|_| anyhow::anyhow!("batch size overflows i64"))?;
    let max_len_i64 =
        i64::try_from(max_len).map_err(|_| anyhow::anyhow!("seq len overflows i64"))?;
    let shape = vec![n_i64, max_len_i64];
    let ids_tensor = TensorRef::from_array_view((shape.clone(), ids.as_slice()))
        .map_err(|e| anyhow::anyhow!("ids tensor: {e}"))?;
    let mask_tensor = TensorRef::from_array_view((shape, mask.as_slice()))
        .map_err(|e| anyhow::anyhow!("attention_mask tensor: {e}"))?;

    let mut session = model
        .session
        .lock()
        .map_err(|_| anyhow::anyhow!("session mutex poisoned"))?;
    let outputs = session
        .run(inputs![
            "input_ids" => ids_tensor,
            "attention_mask" => mask_tensor,
        ])
        .map_err(|e| anyhow::anyhow!("session.run: {e}"))?;

    let logits_value = outputs
        .get("logits")
        .ok_or_else(|| anyhow::anyhow!("model output has no `logits` tensor"))?;
    let logits_view = logits_value
        .try_extract_array::<f32>()
        .map_err(|e| anyhow::anyhow!("extract logits: {e}"))?;
    let logits_flat = logits_view
        .as_slice()
        .ok_or_else(|| anyhow::anyhow!("logits not contiguous"))?;
    let total = logits_flat.len();
    if total == 0 || total % n != 0 {
        anyhow::bail!("logits has unexpected length {total} for batch size {n}");
    }
    let num_classes = total / n;

    let mut out: Vec<Classification> = Vec::with_capacity(n);
    for row_logits in logits_flat.chunks(num_classes) {
        out.push(classify_row(model, row_logits)?);
    }
    Ok(out)
}

/// Build one [`Classification`] from a row of logits: argmax + the top-5
/// candidates, all normalized by one softmax denominator per row —
/// `p(i) = exp(z_i − z_max) / denom`, so the argmax probability
/// (`exp(0)/denom = 1/denom`) and the ranked candidates' probabilities come
/// from the same normalization.
fn classify_row(model: &LoadedModel, row_logits: &[f32]) -> anyhow::Result<Classification> {
    let (argmax, &argmax_val) = row_logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .ok_or_else(|| anyhow::anyhow!("argmax: empty logits row"))?;
    let label = model
        .id2label
        .get(argmax)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("argmax {argmax} out of id2label bounds"))?;
    let denom = softmax_denom(row_logits, argmax_val);
    let mut candidates = Vec::with_capacity(5);
    for i in top5_indices(row_logits) {
        let candidate_label = model
            .id2label
            .get(i)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("top-5 index {i} out of id2label bounds"))?;
        let z = row_logits
            .get(i)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("top-5 index {i} out of logits bounds"))?;
        candidates.push(TopCandidate {
            index: i,
            label: candidate_label,
            probability: (z - argmax_val).exp() / denom,
        });
    }
    let top5: [TopCandidate; 5] = candidates
        .try_into()
        .map_err(|_| anyhow::anyhow!("top-5 candidate build produced wrong arity"))?;
    Ok(Classification {
        label,
        label_index: argmax,
        top5,
        logit_argmax: argmax_val,
        probability: 1.0 / denom,
    })
}

/// The shared softmax denominator `Σ_j exp(z_j − z_max)`, computed once per
/// row so the argmax probability (`1 / denom`) and every ranked candidate's
/// probability (`exp(z_i − z_max) / denom`) normalize identically. This is
/// the numerically stable max-shifted form (see `docs/model-confidence.md`
/// for the exact formula, dependency chain, and research-validity notes):
/// subtracting the row maximum before exponentiating bounds every exponent
/// at ≤ 0, so `exp` can't overflow f32 regardless of logit magnitude; the
/// result is identical to naive softmax in exact arithmetic.
fn softmax_denom(row_logits: &[f32], z_max: f32) -> f32 {
    row_logits.iter().map(|&z| (z - z_max).exp()).sum()
}

/// Top-5 indices, sorted descending by logit value. Ties broken by index order
/// (deterministic across runs), which matches `NumPy`'s stable-sort behavior in
/// `np.argsort`/`np.argpartition` for equal values. Every model has ≥ 48
/// classes, so 5 ranks always exist; a shorter row (impossible in practice)
/// pads with index 0.
fn top5_indices(logits: &[f32]) -> [usize; 5] {
    let mut indexed: Vec<(usize, f32)> = logits.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    std::array::from_fn(|rank| indexed.get(rank).map_or(0, |x| x.0))
}

/// Normalize a model `id2label` value to the canonical zero-padded CCM code
/// used by the official taxonomy (and therefore by `ccm_taxonomy` joins,
/// display, and exports).
///
/// The annamp models' label strings are float-mangled — somewhere in
/// training-data prep the codes were parsed as floats and stringified, losing
/// leading zeros and trailing fractional zeros: `1` (should be `01`), `1.0`
/// (`01.0000`), `11.1` (`11.1000`), `12.041` (`12.0410`). Canonical form is
/// a 2-digit integer part plus, for 4/6-digit levels, a dot and a 2/4-digit
/// fraction: `XX`, `XX.XX`, `XX.XXXX`.
#[must_use]
pub fn normalize_ccm_code(label: &str, digit_level: u8) -> String {
    let (int_part, frac_part) = match label.split_once('.') {
        Some((i, f)) => (i, f),
        None => (label, ""),
    };
    let frac_len: usize = match digit_level {
        4 => 2,
        6 => 4,
        _ => 0,
    };
    if frac_len == 0 {
        format!("{int_part:0>2}")
    } else {
        format!("{int_part:0>2}.{frac_part:0<frac_len$}")
    }
}

#[derive(Deserialize)]
struct HfConfig {
    id2label: HashMap<String, String>,
    /// Family-specific pad token id (`RoBERTa` 1, `ModernBERT` 50283); drives
    /// the tokenizer padding setup in [`load_model`].
    pad_token_id: u32,
}

fn load_hf_config(config_path: &Path) -> anyhow::Result<HfConfig> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", config_path.display()))?;
    serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("parse {}: {e}", config_path.display()))
}

/// Convert `id2label` (string-keyed JSON object) into a Vec where the index is
/// the class id. Indices missing from the map error loudly rather than
/// silently leaving empty slots.
fn id2label_table(cfg: &HfConfig) -> anyhow::Result<Vec<String>> {
    let n = cfg.id2label.len();
    let mut out = vec![String::new(); n];
    for (k, v) in &cfg.id2label {
        let i: usize = k
            .parse()
            .map_err(|e| anyhow::anyhow!("non-integer id2label key {k:?}: {e}"))?;
        if i >= n {
            anyhow::bail!("id2label index {i} out of bounds (have {n} entries)");
        }
        if let Some(slot) = out.get_mut(i) {
            slot.clone_from(v);
        }
    }
    if out.iter().any(String::is_empty) {
        anyhow::bail!("id2label has gaps");
    }
    Ok(out)
}

/// Holder for the three digit-level models, loaded once at app startup.
#[derive(Debug)]
pub struct InferenceRegistry {
    pub two_digit: LoadedModel,
    pub four_digit: LoadedModel,
    pub six_digit: LoadedModel,
}

impl InferenceRegistry {
    /// Pick a loaded model by digit level (2/4/6). Returns `None` for any other
    /// value.
    #[must_use]
    pub fn by_digit_level(&self, level: u8) -> Option<&LoadedModel> {
        match level {
            2 => Some(&self.two_digit),
            4 => Some(&self.four_digit),
            6 => Some(&self.six_digit),
            _ => None,
        }
    }

    /// The execution provider this registry's sessions run on. All three
    /// models load with the same priority list in one `load_all_models` call,
    /// so resolution is uniform by construction; read it from any of them.
    #[must_use]
    pub fn execution_provider(&self) -> EpKind {
        self.two_digit.resolved_ep
    }
}

/// Same product dir convention as `db.rs::PRODUCT_DIR` and `config.rs` —
/// kept duplicated rather than hoisted into a shared module while only two
/// callers exist; promote when a third lands.
const PRODUCT_DIR: &str = "college-course-map";
const MODELS_SUBDIR: &str = "models";

/// Resolve the on-disk model directory for **non-airgap** (dev/connected)
/// runs. The airgap build never calls this — its models live in the bundle's
/// `resource_dir` and `lib.rs` resolves that at setup (see the `airgap`
/// cargo feature).
///
/// Resolution order:
/// 1. `COURSE_CLASSIFIER_MODELS_DIR` env var if set — explicit override for
///    CI and one-off dev tweaks (e.g. pointing at a freshly converted
///    `scripts/models/output/` without copying).
/// 2. `<data>/college-course-map/models/` — the standard location, portable
///    across machines via a copy of the data dir. This is what
///    `task models:install` populates.
pub fn models_root() -> Result<PathBuf, String> {
    if let Ok(env) = std::env::var("COURSE_CLASSIFIER_MODELS_DIR") {
        return Ok(PathBuf::from(env));
    }
    dirs::data_dir()
        .map(|dir| dir.join(PRODUCT_DIR).join(MODELS_SUBDIR))
        .ok_or_else(|| "no platform data directory available".to_owned())
}

/// Load all three digit-level models from `root`. Slow (each model is
/// ~500 MB); call once at startup. The caller resolves `root` per build
/// flavor (`resource_dir` for airgap, [`models_root`] otherwise) and passes
/// the settings' execution-provider priority list + CPU thread cap.
pub fn load_all_models(
    root: &Path,
    eps: &[EpKind],
    max_cpu_threads: u32,
) -> anyhow::Result<InferenceRegistry> {
    if !root.exists() {
        anyhow::bail!(
            "models directory missing: {} — run `task models:install` to copy \
             from scripts/models/output, or set COURSE_CLASSIFIER_MODELS_DIR \
             to point at an existing directory",
            root.display()
        );
    }
    Ok(InferenceRegistry {
        two_digit: load_model(&root.join("two-digit"), 2, eps, max_cpu_threads)?,
        four_digit: load_model(&root.join("four-digit"), 4, eps, max_cpu_threads)?,
        six_digit: load_model(&root.join("six-digit"), 6, eps, max_cpu_threads)?,
    })
}

/// Lazily-populated holder for the loaded registry, managed as Tauri state.
///
/// The connected build boots model-less on first run (files may not be
/// downloaded yet), so commands can no longer assume models exist — they take
/// this store and error with "models not loaded" when empty. Loading happens
/// off the startup path (`models::autoload_if_present` / the `load_models`
/// command); the registry goes behind an `Arc` so a run worker holds its
/// clone for the whole run regardless of later store changes.
#[derive(Debug, Default)]
pub struct ModelStore {
    registry: std::sync::RwLock<Option<std::sync::Arc<InferenceRegistry>>>,
    loading: std::sync::atomic::AtomicBool,
}

impl ModelStore {
    pub fn get(&self) -> Option<std::sync::Arc<InferenceRegistry>> {
        self.registry.read().ok().and_then(|guard| guard.clone())
    }

    pub fn is_loaded(&self) -> bool {
        self.registry
            .read()
            .ok()
            .is_some_and(|guard| guard.is_some())
    }

    pub fn is_loading(&self) -> bool {
        self.loading.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn set(&self, registry: InferenceRegistry) -> Result<(), String> {
        let mut guard = self
            .registry
            .write()
            .map_err(|_| "model store lock poisoned".to_owned())?;
        *guard = Some(std::sync::Arc::new(registry));
        Ok(())
    }

    /// Empty the store so the next load rebuilds sessions (EPI-73: an EP
    /// priority reorder re-registers providers). A run in flight keeps its
    /// `Arc` clone and finishes on the old sessions — new runs get the new
    /// registry.
    pub(crate) fn clear(&self) -> Result<(), String> {
        let mut guard = self
            .registry
            .write()
            .map_err(|_| "model store lock poisoned".to_owned())?;
        *guard = None;
        Ok(())
    }

    /// Claim the loading flag. Returns false if a load is already in flight —
    /// callers must not start a second one.
    pub(crate) fn try_begin_loading(&self) -> bool {
        !self.loading.swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn end_loading(&self) {
        self.loading
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::{FailedEps, normalize_ccm_code, softmax_denom, top5_indices};
    use crate::runtime::EpKind;

    /// A provider that failed once is remembered for the process; others are
    /// unaffected and repeats don't accumulate (EPI-104).
    #[test]
    fn failed_eps_memoises_failures_only() {
        let failed = FailedEps::new();
        assert!(!failed.contains(EpKind::DirectMl));
        failed.record(EpKind::DirectMl);
        failed.record(EpKind::DirectMl);
        assert!(failed.contains(EpKind::DirectMl));
        assert!(!failed.contains(EpKind::Cuda));
        assert_eq!(
            failed
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );
    }

    /// `p(argmax)` in the max-shifted form the pipeline uses.
    fn softmax_at(row_logits: &[f32], z_max: f32) -> f32 {
        1.0 / softmax_denom(row_logits, z_max)
    }

    /// Descending by logit; equal values break ties by index (stable, matches
    /// the Python reference's argsort behavior).
    #[test]
    fn top5_sorts_descending_with_stable_ties() {
        let logits = [1.0_f32, 3.0, 3.0, 2.0, 0.5, 0.1];
        assert_eq!(top5_indices(&logits), [1, 2, 3, 0, 4]);
    }

    /// Candidate probabilities computed off the shared denominator are a
    /// proper distribution slice: rank-1 equals `softmax_at`, ranks are
    /// non-increasing, and the five together never exceed 1.
    #[test]
    fn top5_probabilities_share_the_argmax_normalization() {
        let logits = [2.0_f32, 1.0, 0.5, -0.5, 3.0, 0.0, -1.0];
        let z_max = 3.0_f32;
        let denom = softmax_denom(&logits, z_max);
        let probs: Vec<f32> = top5_indices(&logits)
            .iter()
            .filter_map(|&i| logits.get(i))
            .map(|&z| (z - z_max).exp() / denom)
            .collect();
        let first = probs.first().copied().unwrap_or(0.0);
        assert!((first - softmax_at(&logits, z_max)).abs() < 1e-7);
        assert!(
            probs.windows(2).all(|w| matches!(w, [a, b] if a >= b)),
            "probs = {probs:?}"
        );
        assert!(probs.iter().sum::<f32>() <= 1.0 + 1e-6);
    }

    /// Cases drawn from the real `id2label` sets of all three models —
    /// including every float-mangling shape observed (lost leading zeros,
    /// lost trailing fractional zeros).
    #[test]
    fn normalizes_float_mangled_labels() {
        // 2-digit: XX
        assert_eq!(normalize_ccm_code("1", 2), "01");
        assert_eq!(normalize_ccm_code("10", 2), "10");
        assert_eq!(normalize_ccm_code("45", 2), "45");
        // 4-digit: XX.XX
        assert_eq!(normalize_ccm_code("1.0", 4), "01.00");
        assert_eq!(normalize_ccm_code("1.01", 4), "01.01");
        assert_eq!(normalize_ccm_code("11.1", 4), "11.10");
        // 6-digit: XX.XXXX
        assert_eq!(normalize_ccm_code("1.0", 6), "01.0000");
        assert_eq!(normalize_ccm_code("1.0101", 6), "01.0101");
        assert_eq!(normalize_ccm_code("11.1", 6), "11.1000");
        assert_eq!(normalize_ccm_code("12.041", 6), "12.0410");
        // Already-canonical input is a no-op.
        assert_eq!(normalize_ccm_code("01.0000", 6), "01.0000");
    }

    /// The probability must match naive softmax on friendly values and stay
    /// finite (no overflow) on logit magnitudes that would blow up the naive
    /// form in f32 (`exp(100)` is already infinite).
    #[test]
    fn softmax_at_matches_naive_and_is_stable() {
        let logits = [2.0_f32, 1.0, 0.5];
        let max = 2.0_f32;
        let naive: f32 = logits.iter().map(|z| z.exp()).sum();
        let expected = max.exp() / naive;
        let got = softmax_at(&logits, max);
        assert!((got - expected).abs() < 1e-6, "got {got}, want {expected}");

        // Uniform logits → uniform probability.
        let uniform = [3.0_f32; 4];
        assert!((softmax_at(&uniform, 3.0) - 0.25).abs() < 1e-6);

        // Large-magnitude logits: naive softmax would overflow f32.
        let big = [500.0_f32, 499.0, -500.0];
        let p = softmax_at(&big, 500.0);
        assert!(p.is_finite() && p > 0.5 && p < 1.0, "p = {p}");
    }
}
