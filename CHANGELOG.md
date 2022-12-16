# Changelog

## [2.0.2](https://github.com/cailloumajor/centrifugo-change-stream/compare/v2.0.1...v2.0.2) (2022-12-16)


### Bug Fixes

* **deps:** update dependency centrifuge to 3.1 ([0621dcf](https://github.com/cailloumajor/centrifugo-change-stream/commit/0621dcf2a8be55d67bf9cf381b4029803eed6c7b))
* **deps:** update rust crate clap to 4.0.29 ([811593e](https://github.com/cailloumajor/centrifugo-change-stream/commit/811593e164e46d779d1c08026b42a2ce644bfa35))
* **deps:** update rust crate serde to 1.0.149 ([02346a9](https://github.com/cailloumajor/centrifugo-change-stream/commit/02346a9040fba32ae8e254c61d52c24077aaeb2e))
* **deps:** update rust crate serde to 1.0.150 ([8e5ffd6](https://github.com/cailloumajor/centrifugo-change-stream/commit/8e5ffd6ca265b75ab962ce4f87dbf9d983ddd452))
* **deps:** update rust crate tokio to 1.23.0 ([ed685b8](https://github.com/cailloumajor/centrifugo-change-stream/commit/ed685b80835c93f1f5f6ab377f3da7999409f410))
* **deps:** update rust docker tag to v1.66.0 ([db1c5db](https://github.com/cailloumajor/centrifugo-change-stream/commit/db1c5db29f3c985dd27849d0aec44744038b19d8))

## [2.0.1](https://github.com/cailloumajor/centrifugo-change-stream/compare/v2.0.0...v2.0.1) (2022-11-25)


### Bug Fixes

* **deps:** update rust crate clap to 4.0.27 ([951726c](https://github.com/cailloumajor/centrifugo-change-stream/commit/951726cc39e0c899652142bd016db623bbc1ed98))
* **deps:** update rust crate serde_json to 1.0.89 ([bbd808d](https://github.com/cailloumajor/centrifugo-change-stream/commit/bbd808da765cef8175b3bfab304858c7e2743103))
* trace log events ([f2af097](https://github.com/cailloumajor/centrifugo-change-stream/commit/f2af09790652330916cd925a9606734f271ed3c6))

## [2.0.0](https://github.com/cailloumajor/centrifugo-change-stream/compare/v1.0.0...v2.0.0) (2022-11-20)


### ⚠ BREAKING CHANGES

* switch to dotted Centrifugo namespace

### Features

* switch to dotted Centrifugo namespace ([a56c7f4](https://github.com/cailloumajor/centrifugo-change-stream/commit/a56c7f4f8133ca9f909a7778912e787018b5d822))


### Bug Fixes

* **deps:** update rust crate serde_json to 1.0.88 ([6f68dcf](https://github.com/cailloumajor/centrifugo-change-stream/commit/6f68dcf1101bd5d1eeebc3241f91d841618ea526))
* **deps:** update rust crate tokio to 1.22.0 ([958ec11](https://github.com/cailloumajor/centrifugo-change-stream/commit/958ec11be77a06d650db9c092a77ad82e0186b8c))

## [1.0.0](https://github.com/cailloumajor/centrifugo-change-stream/compare/v0.1.0...v1.0.0) (2022-11-18)


### Features

* implement Centrifugo subscribe proxy ([a483ad1](https://github.com/cailloumajor/centrifugo-change-stream/commit/a483ad1fcdb90b28e41854acaf9a1d61e5feba90))
* implement health API endpoint ([aa0577e](https://github.com/cailloumajor/centrifugo-change-stream/commit/aa0577e58e3e00094001f58f88c9ca28af1530b7))
* implement health service ([ebd556d](https://github.com/cailloumajor/centrifugo-change-stream/commit/ebd556d389546ecae5f0993f6198b03588813920))
* implement healthcheck binary ([073fbf7](https://github.com/cailloumajor/centrifugo-change-stream/commit/073fbf78fcdbda438f9ceacc5e92c80dd1eeea04))
* implement termination signals handling ([d8eaafe](https://github.com/cailloumajor/centrifugo-change-stream/commit/d8eaafee849df95eb033b325d389ad9ab4a92a45))


### Bug Fixes

* add default value for Centrifugo API URL ([74e331e](https://github.com/cailloumajor/centrifugo-change-stream/commit/74e331e0aebebbcff35298b4cf3e865077c1ca69))
* **deps:** update rust crate clap to 4.0.26 ([41f6931](https://github.com/cailloumajor/centrifugo-change-stream/commit/41f6931c46fac72f25399ef49873219804f6bcb1))
* do not install unneeded libssl-dev in image ([69f4ce6](https://github.com/cailloumajor/centrifugo-change-stream/commit/69f4ce6c8adc76f69912893f7685c27abb4ca7cb))
* handle change stream error item ([f74a841](https://github.com/cailloumajor/centrifugo-change-stream/commit/f74a8413f856a2dfd95fd8180c5d6963567ddbdf))
* implement traceable error type ([7f81650](https://github.com/cailloumajor/centrifugo-change-stream/commit/7f8165030faf03de94b929536cdbcdcd9e7b0c74))
* remove unused code ([36c2315](https://github.com/cailloumajor/centrifugo-change-stream/commit/36c2315fa7e7a30ac489bf24aecf294233e1cf89))
* **tests:** add MongoDB initialization retry ([5bbfca9](https://github.com/cailloumajor/centrifugo-change-stream/commit/5bbfca9a22459731c604e21011cffba795cfc0e6))
* use idiomatic trait ([32f5e32](https://github.com/cailloumajor/centrifugo-change-stream/commit/32f5e32f0f38e6acce7eb55a36740b19114ef1aa))

## 0.1.0 (2022-11-09)


### Features

* initial and partial implementation ([3e231be](https://github.com/cailloumajor/centrifugo-change-stream/commit/3e231be63e4a7e8fdea203fb5d40f74119ae471f))


### Bug Fixes

* **deps:** update rust docker tag to v1.65.0 ([33d5166](https://github.com/cailloumajor/centrifugo-change-stream/commit/33d5166dccf39dc8e673083adfc626149c595144))
