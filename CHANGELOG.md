# Changelog

## [0.6.0](https://github.com/fspp-epidept/college-course-map/compare/course-classifier-v0.5.0...course-classifier-v0.6.0) (2026-10-06)


### Features

* add CCM Reference viewer and bundled 2010 CCM report ([#190](https://github.com/fspp-epidept/college-course-map/issues/190)) ([4dcf6b7](https://github.com/fspp-epidept/college-course-map/commit/4dcf6b77c72492746d04454a2bf52791f21d274a))
* back up the database before upgrades and refuse newer data ([#245](https://github.com/fspp-epidept/college-course-map/issues/245)) ([73eb573](https://github.com/fspp-epidept/college-course-map/commit/73eb5730e05e8e19b75e5071ccfca30a100e9062))
* CSV import pre-flight with encoding detection ([#182](https://github.com/fspp-epidept/college-course-map/issues/182)) ([1b5d202](https://github.com/fspp-epidept/college-course-map/commit/1b5d2022f26cc2a1dbcfd59a35d98d698a1e1330))
* delete data and reclaim disk space ([#246](https://github.com/fspp-epidept/college-course-map/issues/246)) ([cfc5da2](https://github.com/fspp-epidept/college-course-map/commit/cfc5da2d872840ccb9dc656071d5b9a8000264bd))
* filter the courses table by field, column and CCM code ([#256](https://github.com/fspp-epidept/college-course-map/issues/256)) ([e346be7](https://github.com/fspp-epidept/college-course-map/commit/e346be79b0cb240ef5b1ecafb20b7deea96e81f4)), closes [#254](https://github.com/fspp-epidept/college-course-map/issues/254)
* fold runs into the dataset page ([#248](https://github.com/fspp-epidept/college-course-map/issues/248)) ([b5b8c87](https://github.com/fspp-epidept/college-course-map/commit/b5b8c871259757863399e4ce9ff5a6af81a8f2fe))
* hold an instance lock on the data dir at startup ([#234](https://github.com/fspp-epidept/college-course-map/issues/234)) ([24e833d](https://github.com/fspp-epidept/college-course-map/commit/24e833d5104f90f8a98a1c7ef33ae3554dcf0450)), closes [#233](https://github.com/fspp-epidept/college-course-map/issues/233)
* keep the input profile on imported datasets ([#221](https://github.com/fspp-epidept/college-course-map/issues/221)) ([8ee9a38](https://github.com/fspp-epidept/college-course-map/commit/8ee9a38c58e657c5521a2b436ffbe97a6f1f5654))
* open the window first and show a boot screen ([#241](https://github.com/fspp-epidept/college-course-map/issues/241)) ([8e6e1cb](https://github.com/fspp-epidept/college-course-map/commit/8e6e1cb856839adecccf0a4eca27e636454111d7))
* pick the duplicate key in the columns table ([#258](https://github.com/fspp-epidept/college-course-map/issues/258)) ([fe37b17](https://github.com/fspp-epidept/college-course-map/commit/fe37b17815a0874729c160ad1153c8a702da3f7a)), closes [#254](https://github.com/fspp-epidept/college-course-map/issues/254)
* profile subject, catalog, and title before import ([#220](https://github.com/fspp-epidept/college-course-map/issues/220)) ([094bc12](https://github.com/fspp-epidept/college-course-map/commit/094bc129f7fe955bbd04a656d5664457e0390166))
* record every dataset's column layout on the dataset ([#255](https://github.com/fspp-epidept/college-course-map/issues/255)) ([e50c486](https://github.com/fspp-epidept/college-course-map/commit/e50c4867da760c8ef4adf06b600606f1e64720fa)), closes [#254](https://github.com/fspp-epidept/college-course-map/issues/254)
* replace runs with classification state on the dataset ([#253](https://github.com/fspp-epidept/college-course-map/issues/253)) ([98f8df4](https://github.com/fspp-epidept/college-course-map/commit/98f8df4b4f0c0db98c1251f50f38f65ebc3e08e7))
* reset app data in-app and on Windows uninstall ([#236](https://github.com/fspp-epidept/college-course-map/issues/236)) ([3ab93ac](https://github.com/fspp-epidept/college-course-map/commit/3ab93ac1b22922633dd86474cf673ab4b7cdb90b))
* save filtered rows from one or more datasets as a dataset ([#257](https://github.com/fspp-epidept/college-course-map/issues/257)) ([04ebb2a](https://github.com/fspp-epidept/college-course-map/commit/04ebb2a9fdbecdaa67c4456047c3c5d035b0ead7)), closes [#254](https://github.com/fspp-epidept/college-course-map/issues/254)
* show model inputs and warnings before classifying ([#219](https://github.com/fspp-epidept/college-course-map/issues/219)) ([98ee45f](https://github.com/fspp-epidept/college-course-map/commit/98ee45f5d7d035b84c30a9b4447b8a82d3589177))
* sign and notarize macOS release builds ([#191](https://github.com/fspp-epidept/college-course-map/issues/191)) ([74a53ca](https://github.com/fspp-epidept/college-course-map/commit/74a53ca8a34eeceaa48579f30404884881493b4f))


### Bug Fixes

* drop fabricated 4-digit CCM titles from export and results ([#181](https://github.com/fspp-epidept/college-course-map/issues/181)) ([2471f4e](https://github.com/fspp-epidept/college-course-map/commit/2471f4eee565d6967733251114e7339ceb5fb5b7))
* focus the running app on a second launch ([#223](https://github.com/fspp-epidept/college-course-map/issues/223)) ([20d4d40](https://github.com/fspp-epidept/college-course-map/commit/20d4d40d8708d5c99eaddbef5f154b3d8b881d73))
* keep large app data out of the Windows Roaming profile ([#235](https://github.com/fspp-epidept/college-course-map/issues/235)) ([b82aece](https://github.com/fspp-epidept/college-course-map/commit/b82aecea6a0630e18dd04f2a9c1c2d31efbdde58))
* keep threads out of ONNX Runtime when the app exits ([#251](https://github.com/fspp-epidept/college-course-map/issues/251)) ([6ace4d4](https://github.com/fspp-epidept/college-course-map/commit/6ace4d404f453c338366bad7db255d37570448f1))
* mark imports interrupted by a quit or crash as failed ([#240](https://github.com/fspp-epidept/college-course-map/issues/240)) ([b1eff9a](https://github.com/fspp-epidept/college-course-map/commit/b1eff9a06b434ea3632dbf9e5c83671b3dad70c4))
* route every macOS native menu item to its in-app action ([#222](https://github.com/fspp-epidept/college-course-map/issues/222)) ([4dbb90d](https://github.com/fspp-epidept/college-course-map/commit/4dbb90d62bd44bc253fe78280d432cfa9875717f))
* show startup failures and unclean exits to the user ([#252](https://github.com/fspp-epidept/college-course-map/issues/252)) ([f968508](https://github.com/fspp-epidept/college-course-map/commit/f96850811d0ba55dc79a0599bd498fe79bb434ce))
* shut down cleanly on termination signals ([#228](https://github.com/fspp-epidept/college-course-map/issues/228)) ([914c2ae](https://github.com/fspp-epidept/college-course-map/commit/914c2ae3cbfe304c569eea5280fa6abd525c53c6))


### Performance Improvements

* seed the CCM taxonomy once, through an Appender ([#250](https://github.com/fspp-epidept/college-course-map/issues/250)) ([0039806](https://github.com/fspp-epidept/college-course-map/commit/0039806b590d7da20b2209054f5dc5cfc4b7c90c))

## [0.5.0](https://github.com/fspp-epidept/college-course-map/compare/course-classifier-v0.4.0...course-classifier-v0.5.0) (2026-08-26)


### Features

* CoreML in MLProgram format as an opt-in macOS experiment ([#174](https://github.com/fspp-epidept/college-course-map/issues/174)) ([5bebf2a](https://github.com/fspp-epidept/college-course-map/commit/5bebf2afa1fb55b0918ccb9e0d626364e09b5d90))
* diagnostic log file with Open logs folder in About ([#173](https://github.com/fspp-epidept/college-course-map/issues/173)) ([c55a937](https://github.com/fspp-epidept/college-course-map/commit/c55a93765206bf2e64adc6955ab3ae9d15494a89))
* rewrite fp32 Neg to Mul(-1) in ONNX export for CoreML ([#175](https://github.com/fspp-epidept/college-course-map/issues/175)) ([7e98815](https://github.com/fspp-epidept/college-course-map/commit/7e98815523ea742e1854bb3b7be4b0e9a9e5550c))


### Bug Fixes

* never attempt execution providers the runtime pack lacks ([#168](https://github.com/fspp-epidept/college-course-map/issues/168)) ([008dd1b](https://github.com/fspp-epidept/college-course-map/commit/008dd1be251938d7a0ab2db988695c204ed79e62))
* stop attempting CoreML on macOS until it is validated ([#172](https://github.com/fspp-epidept/college-course-map/issues/172)) ([806ab05](https://github.com/fspp-epidept/college-course-map/commit/806ab056ee71c545383a2f930e0607bbdba927e3))
* survive an unreplayable DuckDB WAL at startup ([#169](https://github.com/fspp-epidept/college-course-map/issues/169)) ([77c3654](https://github.com/fspp-epidept/college-course-map/commit/77c3654bfa4f079254e4384e30cd6328f7803158))

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
