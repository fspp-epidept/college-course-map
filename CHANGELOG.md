# Changelog

## [0.4.0](https://github.com/fspp-epidept/college-course-map/compare/course-classifier-v0.3.0...course-classifier-v0.4.0) (2026-07-30)


### Features

* combined multi-level CSV export and unique-rows mode ([#155](https://github.com/fspp-epidept/college-course-map/issues/155)) ([0c73f18](https://github.com/fspp-epidept/college-course-map/commit/0c73f18341d7a160b820efb839ed2a47c00c3e3b))
* GPU inference via load-dynamic runtime packs ([#151](https://github.com/fspp-epidept/college-course-map/issues/151)) ([62354a5](https://github.com/fspp-epidept/college-course-map/commit/62354a53a28b665f85dbe21fb94615c83e9001bf))
* round-trip CSV export, ccm columns, titles, top-5 candidates ([#153](https://github.com/fspp-epidept/college-course-map/issues/153)) ([330c47e](https://github.com/fspp-epidept/college-course-map/commit/330c47ead4d2bf870ca8d466ce970bf06accbd36))


### Bug Fixes

* disable WebKitGTK DMABUF renderer in shipped Linux builds ([#161](https://github.com/fspp-epidept/college-course-map/issues/161)) ([57b47a6](https://github.com/fspp-epidept/college-course-map/commit/57b47a6704e8b06e2e010ac41875186377211f2d))
* model download concurrency guard, integrity verify, repair path ([#158](https://github.com/fspp-epidept/college-course-map/issues/158)) ([a6696cb](https://github.com/fspp-epidept/college-course-map/commit/a6696cb82c6247442bb2f03247f4b505ae6de371))

## [0.3.0](https://github.com/fspp-epidept/college-course-map/compare/course-classifier-v0.2.0...course-classifier-v0.3.0) (2026-07-03)


### Features

* redesign classify flow — coverage, inline confirm ([#145](https://github.com/fspp-epidept/college-course-map/issues/145)) ([53a7459](https://github.com/fspp-epidept/college-course-map/commit/53a7459760287a1bea411f18ed9ed0ad2c6d5e22))
* run resume, crash sweep, resumability surfacing ([#149](https://github.com/fspp-epidept/college-course-map/issues/149)) ([6758527](https://github.com/fspp-epidept/college-course-map/commit/675852765365e49dc7178a8afee8af2440bb5e37))
* VS Code-style tab context menu ([#147](https://github.com/fspp-epidept/college-course-map/issues/147)) ([311f878](https://github.com/fspp-epidept/college-course-map/commit/311f878e3b9c7f9ae90c82a3485435448a118fa5))

## [0.2.0](https://github.com/fspp-epidept/college-course-map/compare/course-classifier-v0.1.0...course-classifier-v0.2.0) (2026-07-03)


### Features

* connected build — async model loading + first-run HF download ([#137](https://github.com/fspp-epidept/college-course-map/issues/137)) ([82d3e82](https://github.com/fspp-epidept/college-course-map/commit/82d3e8233486346c8a1cdf8ff063db00b1e1b4ab))


### Bug Fixes

* confirm before starting a classification run, drop demo copy ([#144](https://github.com/fspp-epidept/college-course-map/issues/144)) ([70d3206](https://github.com/fspp-epidept/college-course-map/commit/70d3206d933945bf24f50d9b17a140de81706728))
* rate-limit model download progress events, add speed readout ([#142](https://github.com/fspp-epidept/college-course-map/issues/142)) ([f47b1e9](https://github.com/fspp-epidept/college-course-map/commit/f47b1e949f7108a9ec713b42465d23e2c1250d31))
