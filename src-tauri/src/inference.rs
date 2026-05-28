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
    inputs,
    session::{Session, builder::GraphOptimizationLevel},
    value::TensorRef,
};
use serde::Deserialize;
use tokenizers::{Tokenizer, TruncationDirection, TruncationParams};

/// Matches `max_length=512` in `scripts/models/_lib/inference.py::predict_batch`.
const MAX_SEQ_LEN: usize = 512;

/// A model loaded into ONNX Runtime with its tokenizer + class label table.
pub struct LoadedModel {
    pub digit_level: u8,
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

/// One classification: argmax label, its index, the top-3 indices (sorted
/// descending by logit), and the raw logit value at argmax.
#[derive(Debug, Clone)]
pub struct Classification {
    pub label: String,
    pub label_index: usize,
    pub top3: [usize; 3],
    pub logit_argmax: f32,
}

/// Build a [`LoadedModel`] from a directory containing `model.onnx`,
/// `tokenizer.json`, and `config.json`.
pub fn load_model(model_dir: &Path, digit_level: u8) -> anyhow::Result<LoadedModel> {
    let mut tokenizer = Tokenizer::from_file(model_dir.join("tokenizer.json"))
        .map_err(|e| anyhow::anyhow!("load tokenizer: {e}"))?;
    // Match the Python pipeline: `truncation=True, max_length=512`. Padding is
    // not configured: single-input inference doesn't need it, and batched
    // inference will set padding per-batch when it lands.
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: MAX_SEQ_LEN,
            direction: TruncationDirection::Right,
            ..Default::default()
        }))
        .map_err(|e| anyhow::anyhow!("set truncation: {e}"))?;

    let id2label = load_id2label(&model_dir.join("config.json"))?;

    // ort's `Error` carries a builder phantom that's not `Send + Sync`, so we
    // can't `?` it into `anyhow::Error`; stringify at the boundary.
    let session = Session::builder()
        .map_err(|e| anyhow::anyhow!("session builder: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow::anyhow!("set opt level: {e}"))?
        .commit_from_file(model_dir.join("model.onnx"))
        .map_err(|e| anyhow::anyhow!("commit_from_file: {e}"))?;

    Ok(LoadedModel {
        digit_level,
        session: Mutex::new(session),
        tokenizer,
        id2label,
    })
}

/// Run one input through the model. Tokenizes, builds `input_ids` +
/// `attention_mask` tensors, runs inference, returns the argmax / top-3 /
/// logit-at-argmax.
pub fn classify(model: &LoadedModel, input: &str) -> anyhow::Result<Classification> {
    let encoding = model
        .tokenizer
        .encode(input, true)
        .map_err(|e| anyhow::anyhow!("encode: {e}"))?;
    let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| i64::from(id)).collect();
    let mask: Vec<i64> = encoding
        .get_attention_mask()
        .iter()
        .map(|&m| i64::from(m))
        .collect();
    let seq_len_i64 = i64::try_from(ids.len())
        .map_err(|_| anyhow::anyhow!("token sequence length overflows i64"))?;

    let ids_tensor = TensorRef::from_array_view((vec![1_i64, seq_len_i64], ids.as_slice()))
        .map_err(|e| anyhow::anyhow!("ids tensor: {e}"))?;
    let mask_tensor = TensorRef::from_array_view((vec![1_i64, seq_len_i64], mask.as_slice()))
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

    // The annamp models export with a single output named "logits" of shape
    // [1, num_classes]. Pull it by name so a future model that adds extra
    // outputs (e.g. hidden states) doesn't shuffle the index.
    let logits_value = outputs
        .get("logits")
        .ok_or_else(|| anyhow::anyhow!("model output has no `logits` tensor"))?;
    let logits_view = logits_value
        .try_extract_array::<f32>()
        .map_err(|e| anyhow::anyhow!("extract logits: {e}"))?;
    let logits = logits_view
        .as_slice()
        .ok_or_else(|| anyhow::anyhow!("logits not contiguous"))?;
    let num_classes = logits.len();
    if num_classes == 0 {
        anyhow::bail!("model returned empty logits");
    }

    // argmax + top-3 in one pass: stable, no allocation beyond a fixed array.
    let (argmax, &argmax_val) = logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .ok_or_else(|| anyhow::anyhow!("argmax: empty logits"))?;
    let top3 = top3_indices(logits);

    let label = model
        .id2label
        .get(argmax)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("argmax {argmax} out of id2label bounds"))?;

    Ok(Classification {
        label,
        label_index: argmax,
        top3,
        logit_argmax: argmax_val,
    })
}

/// Top-3 indices, sorted descending by logit value. Ties broken by index order
/// (deterministic across runs), which matches `NumPy`'s stable-sort behavior in
/// `np.argsort`/`np.argpartition` for equal values.
fn top3_indices(logits: &[f32]) -> [usize; 3] {
    let mut indexed: Vec<(usize, f32)> = logits.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    [
        indexed.first().map_or(0, |x| x.0),
        indexed.get(1).map_or(0, |x| x.0),
        indexed.get(2).map_or(0, |x| x.0),
    ]
}

#[derive(Deserialize)]
struct HfConfig {
    id2label: HashMap<String, String>,
}

/// Parse `config.json` `id2label` (string-keyed JSON object) into a Vec where
/// the index is the class id. Indices missing from the map error loudly rather
/// than silently leaving empty slots.
fn load_id2label(config_path: &Path) -> anyhow::Result<Vec<String>> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", config_path.display()))?;
    let cfg: HfConfig = serde_json::from_str(&raw)?;
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
}

/// Resolve the on-disk model directory for the spike. For now this is the
/// Python pipeline's output dir; the bundled-resource lookup lands when #52
/// wires up the build-time model embed.
#[must_use]
pub fn models_root() -> PathBuf {
    if let Ok(env) = std::env::var("COURSE_CLASSIFIER_MODELS_DIR") {
        return PathBuf::from(env);
    }
    // CARGO_MANIFEST_DIR is src-tauri/; the converted ONNX trees live under
    // ../scripts/models/output during the spike.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("models")
        .join("output")
}

/// Load all three digit-level models from the pipeline output dir. Slow
/// (each model is ~500 MB); call once at startup.
pub fn load_all_models() -> anyhow::Result<InferenceRegistry> {
    let root = models_root();
    Ok(InferenceRegistry {
        two_digit: load_model(&root.join("two-digit"), 2)?,
        four_digit: load_model(&root.join("four-digit"), 4)?,
        six_digit: load_model(&root.join("six-digit"), 6)?,
    })
}
