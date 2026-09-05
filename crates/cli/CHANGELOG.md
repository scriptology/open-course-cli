# Changelog

## [0.21.0](https://github.com/scriptology/open-course-cli/compare/v0.20.1...v0.21.0) (2026-09-05)


### Features

* **llm:** disable thinking for all Anthropic-family providers ([9c28fb9](https://github.com/scriptology/open-course-cli/commit/9c28fb943d0f0726d3711c58272d7eba164e49e4))

## [0.20.1](https://github.com/scriptology/open-course-cli/compare/v0.20.0...v0.20.1) (2026-08-26)


### Bug Fixes

* **llm:** route minimax through the Anthropic-compatible API ([e762e28](https://github.com/scriptology/open-course-cli/commit/e762e288e22705143c62012e7b1daa0b8a4d4584))
* **llm:** route minimax through the Anthropic-compatible API ([fe31287](https://github.com/scriptology/open-course-cli/commit/fe31287bf2cc6f1e615416ed5bbfdcdcbe98e165))

## [0.20.0](https://github.com/scriptology/open-course-cli/compare/v0.19.0...v0.20.0) (2026-08-25)


### Features

* add MiniMax as a built-in provider ([18cdd94](https://github.com/scriptology/open-course-cli/commit/18cdd94cff5450bee34549286742c442d555e36e))
* trigger release for the MiniMax provider ([b77a7cd](https://github.com/scriptology/open-course-cli/commit/b77a7cd650768f53e5d14477b462640e6c28934a))

## [0.19.0](https://github.com/scriptology/open-course-cli/compare/v0.18.3...v0.19.0) (2026-08-25)


### Features

* **core:** add cloze-with-word-bank session stage to the LLM contract ([1863039](https://github.com/scriptology/open-course-cli/commit/1863039a8d1425d00a68c076982f1f390e308c3c))
* **core:** add cloze-with-word-bank session stage to the LLM contract ([c3f5c1c](https://github.com/scriptology/open-course-cli/commit/c3f5c1ce25a4903a7250da66f4113f7695e234f4))

## [0.18.3](https://github.com/scriptology/open-course-cli/compare/v0.18.2...v0.18.3) (2026-08-24)


### Bug Fixes

* **llm:** use max_completion_tokens for the OpenAI provider ([b4da17f](https://github.com/scriptology/open-course-cli/commit/b4da17f9099b78b2a5dbf98da85a66d0af957193))
* **llm:** use max_completion_tokens for the OpenAI provider ([ecfe14f](https://github.com/scriptology/open-course-cli/commit/ecfe14f3f0c894c938e9b313a3307418b44732f5))

## [0.18.2](https://github.com/scriptology/open-course-cli/compare/v0.18.1...v0.18.2) (2026-08-21)


### Bug Fixes

* **cli:** support Ctrl+U/W/K kill shortcuts in text inputs ([cc69ae6](https://github.com/scriptology/open-course-cli/commit/cc69ae64b5b99748f872e01a6a2eb494733c4b35))
* **cli:** support Ctrl+U/W/K kill shortcuts in text inputs ([878d9ca](https://github.com/scriptology/open-course-cli/commit/878d9caca43809f7022e9ba97f5c83cb138169d3))

## [0.18.1](https://github.com/scriptology/open-course-cli/compare/v0.18.0...v0.18.1) (2026-08-20)


### Bug Fixes

* **prompts:** anchor vocabulary extraction to expectedTranslation ([98fb496](https://github.com/scriptology/open-course-cli/commit/98fb496c0ee46222a79cb92bf3f0f84a578f7376))
* **prompts:** anchor vocabulary extraction to expectedTranslation ([5e153a6](https://github.com/scriptology/open-course-cli/commit/5e153a600177a5ca38e9e6c3abdb5390823844b4))

## [0.18.0](https://github.com/scriptology/open-course-cli/compare/v0.17.0...v0.18.0) (2026-08-20)


### Features

* **llm:** support enable_thinking option for OpenAI-compatible providers ([096a1b6](https://github.com/scriptology/open-course-cli/commit/096a1b6c506327ca67bfa7288671e59d3870e16c))

## [0.17.0](https://github.com/scriptology/open-course-cli/compare/v0.16.2...v0.17.0) (2026-08-17)


### Features

* **cli:** persistent error screen when session generation fails ([2a09502](https://github.com/scriptology/open-course-cli/commit/2a09502e937de1713d66a954b017405dd4bc3947))
* **core:** demand native/target language separation in exercise prompt ([9fc8053](https://github.com/scriptology/open-course-cli/commit/9fc8053f829abc10709a3ceb8127310600418bc1))

## [0.16.2](https://github.com/scriptology/open-course-cli/compare/v0.16.1...v0.16.2) (2026-08-16)


### Bug Fixes

* **vocabulary:** track conjunctions as content vocabulary ([5404db0](https://github.com/scriptology/open-course-cli/commit/5404db057054c6388a8f5d73f4a32a0ed01ff648))

## [0.16.1](https://github.com/scriptology/open-course-cli/compare/v0.16.0...v0.16.1) (2026-08-16)


### Bug Fixes

* **prompts:** expand language codes to names in prompt prose ([1b8be3f](https://github.com/scriptology/open-course-cli/commit/1b8be3fcd536cbe367cba17e9bfd04dddea3b447))

## [0.16.0](https://github.com/scriptology/open-course-cli/compare/v0.15.1...v0.16.0) (2026-08-15)


### Features

* **vocabulary:** preview new words and rotate forced review vocabulary ([ea2de30](https://github.com/scriptology/open-course-cli/commit/ea2de3048d0e5d12395dcb63c072ef9824553e2e))

## [0.15.1](https://github.com/scriptology/open-course-cli/compare/v0.15.0...v0.15.1) (2026-08-14)


### Bug Fixes

* **vocabulary:** apply the warm-up card cap after skipping untranslated lemmas ([54b7717](https://github.com/scriptology/open-course-cli/commit/54b77174430266873650b2422b74cf4d464cf911))
* **vocabulary:** apply the warm-up card cap after skipping untranslated lemmas ([559dd4d](https://github.com/scriptology/open-course-cli/commit/559dd4d8736cf4f814fd1df6f9ab9497f545fb8e))
* **vocabulary:** first session contact moves words new-&gt;practicing ([a2585c0](https://github.com/scriptology/open-course-cli/commit/a2585c094dc4ad34bce4bbe7150239bf66383554))
* **vocabulary:** first session contact moves words new-&gt;practicing ([cec4dc6](https://github.com/scriptology/open-course-cli/commit/cec4dc6a44d8761bb8911f80aa7dc04327c7ea8f))

## [0.15.0](https://github.com/scriptology/open-course-cli/compare/v0.14.0...v0.15.0) (2026-08-14)


### Features

* **core:** show warmup cards only for words present in exercises ([c6e5aae](https://github.com/scriptology/open-course-cli/commit/c6e5aae6fb8487bffdbf6395e5e99682dc007d09))

## [0.14.0](https://github.com/scriptology/open-course-cli/compare/v0.13.0...v0.14.0) (2026-08-13)


### Features

* session word warm-up before exercises ([0c951e2](https://github.com/scriptology/open-course-cli/commit/0c951e2ac324c40e368d736bd7ea68ba7d55b81d))
* session word warm-up before exercises ([484b2a9](https://github.com/scriptology/open-course-cli/commit/484b2a9bc9dc5979ab88e380efb2601ee9c0c8d6))

## [0.13.0](https://github.com/scriptology/open-course-cli/compare/v0.12.0...v0.13.0) (2026-08-13)


### Features

* vocabulary system with lemmas, UD forms, and CEFR levels ([86093b5](https://github.com/scriptology/open-course-cli/commit/86093b58f837aa209727e8c995e728f95a71ed4e))
* vocabulary system with lemmas, UD forms, and CEFR levels ([552dda5](https://github.com/scriptology/open-course-cli/commit/552dda536ca1378c4e3934716b288f33a2561e34))

## [0.12.0](https://github.com/scriptology/open-course-cli/compare/v0.11.6...v0.12.0) (2026-08-12)


### Features

* **core:** include response excerpt in curriculum parse errors ([5000e34](https://github.com/scriptology/open-course-cli/commit/5000e34cfc981d88f8f98486f91e7104b9aa10fa))
* **core:** include response excerpt in curriculum parse errors ([4a1fd54](https://github.com/scriptology/open-course-cli/commit/4a1fd54383637161c85cf77637f583ee51a453e3))

## [0.11.6](https://github.com/scriptology/open-course-cli/compare/v0.11.5...v0.11.6) (2026-08-12)


### Bug Fixes

* course progress percent is the share of completed topics ([0e14d75](https://github.com/scriptology/open-course-cli/commit/0e14d75e09daeea256d3b518768118bef55a367a))
* course progress percent is the share of completed topics ([2ff3073](https://github.com/scriptology/open-course-cli/commit/2ff30732d4189451c6c9c8bc83b882846da60483))

## [0.11.5](https://github.com/scriptology/open-course-cli/compare/v0.11.4...v0.11.5) (2026-08-08)


### Bug Fixes

* **llm:** anchor exercise complexity to the topic's CEFR level ([a246cd1](https://github.com/scriptology/open-course-cli/commit/a246cd16e1ae1a8f4518f02437979e7abba84803))

## [0.11.4](https://github.com/scriptology/open-course-cli/compare/v0.11.3...v0.11.4) (2026-08-07)


### Bug Fixes

* **cli:** sync every pair on manual "Sync now", not just the active one ([6601c7c](https://github.com/scriptology/open-course-cli/commit/6601c7c725fd7c393c7d45fd4a766c976f9ea93d))

## [0.11.3](https://github.com/scriptology/open-course-cli/compare/v0.11.2...v0.11.3) (2026-08-06)


### Bug Fixes

* render low-activity calendar days like quiet days ([69a55cd](https://github.com/scriptology/open-course-cli/commit/69a55cdb593494470ce47ce08e73756a91f81765))
* render low-activity calendar days like quiet days ([d834fd5](https://github.com/scriptology/open-course-cli/commit/d834fd5849f18e96d39cb1483b9f66adb2da47c7))

## [0.11.2](https://github.com/scriptology/open-course-cli/compare/v0.11.1...v0.11.2) (2026-08-06)


### Bug Fixes

* gate next-topic and side-topic selection by CEFR level ladder ([f209f52](https://github.com/scriptology/open-course-cli/commit/f209f52ecbf71061389bb9588a79f91ecf82ff42))
* подбор следующей темы и сайд-тем с учётом CEFR-лесенки ([e7c1ceb](https://github.com/scriptology/open-course-cli/commit/e7c1cebe0f00bbaf0eeb64b99ff97d5ea3d40d65))

## [0.11.1](https://github.com/scriptology/open-course-cli/compare/v0.11.0...v0.11.1) (2026-08-04)


### Bug Fixes

* **cli:** upload pre-sync data on first bind (backfill) ([afa13e3](https://github.com/scriptology/open-course-cli/commit/afa13e35c8897b2aaebdf64f39dd2ed7ec3f8c99))
* **cli:** upload pre-sync data on first bind (backfill) ([fec8153](https://github.com/scriptology/open-course-cli/commit/fec81536ec1f20c5b082134f24d1b88210580129))

## [0.11.0](https://github.com/scriptology/open-course-cli/compare/v0.10.1...v0.11.0) (2026-08-03)


### Features

* **cli:** sync all pairs automatically with a merge-based bind ([32325d0](https://github.com/scriptology/open-course-cli/commit/32325d056c794b2c65a223f747c56323be2746ed))
* **cli:** автосинк всех пар с merge-bind и токеном в auth.json ([c142df1](https://github.com/scriptology/open-course-cli/commit/c142df19bed064ebbbc396605845f942a34c3945))

## [0.10.1](https://github.com/scriptology/open-course-cli/compare/v0.10.0...v0.10.1) (2026-08-03)


### Bug Fixes

* **cli:** make the Account section selector move and tidy the screen ([752cfa5](https://github.com/scriptology/open-course-cli/commit/752cfa5bf8f0df489a567611ef8d1a368afb2130))
* **cli:** make the Account section selector move and tidy the screen ([30e5f23](https://github.com/scriptology/open-course-cli/commit/30e5f237fe981631cff24e06706072801c2e5590))

## [0.10.0](https://github.com/scriptology/open-course-cli/compare/v0.9.2...v0.10.0) (2026-08-02)


### Features

* **cli:** document where the llm prompts/parsers live ([5d301a0](https://github.com/scriptology/open-course-cli/commit/5d301a0010c4fd89cf165f4b9462fd6b0e3555de))
* **cli:** trigger release 0.10.0 for the core LLM move ([34711be](https://github.com/scriptology/open-course-cli/commit/34711beffab8cda3fb0e4eb24bab6477f89f83e3))

## [0.9.2](https://github.com/scriptology/open-course-cli/compare/v0.9.1...v0.9.2) (2026-08-02)


### Bug Fixes

* one error style across the CLI, action before error in Account ([56791f8](https://github.com/scriptology/open-course-cli/commit/56791f81323313066c67095bfd908ba3361bb5ff))
* one error style across the CLI, action before error in Account ([c50a4e5](https://github.com/scriptology/open-course-cli/commit/c50a4e5f43abf146535596cf50c069a60aaf7d1d))

## [0.9.1](https://github.com/scriptology/open-course-cli/compare/v0.9.0...v0.9.1) (2026-08-01)


### Bug Fixes

* release binaries never built, false "Updated" on failed download ([7a94e2c](https://github.com/scriptology/open-course-cli/commit/7a94e2c0caa841ba307bc618f98197f1ab1cfd0e))
* release binaries never built, false "Updated" on failed download ([f89fbc7](https://github.com/scriptology/open-course-cli/commit/f89fbc7ff1b5986e08e1181cc0ca212021b115f7))

## [0.9.0](https://github.com/scriptology/open-course-cli/compare/v0.8.0...v0.9.0) (2026-08-01)


### Features

* optional cloud sync — workspace split, sync client, account UI ([be63a52](https://github.com/scriptology/open-course-cli/commit/be63a52170c5e7526a5e34d9d19647e9114f988c))
* optional cloud sync — workspace split, sync client, account UI ([001bc39](https://github.com/scriptology/open-course-cli/commit/001bc39cb0f2042c5e8e7aad51781b5b4ac7b8ee))

## [0.8.0](https://github.com/scriptology/open-course-cli/compare/v0.7.0...v0.8.0) (2026-07-30)


### Features

* **ui:** render provider host/API key steps as caret input boxes, endpoint as selector ([3209b0c](https://github.com/scriptology/open-course-cli/commit/3209b0c7fdd53e694e48a0bd597119e6eb278d49))
* **ui:** rework dashboard/curriculum/docs hotkeys and hide wheel/m hints ([f49a96c](https://github.com/scriptology/open-course-cli/commit/f49a96ccd1cc43615520a1d009642da46c27bc95))


### Bug Fixes

* **ui:** reflow footers and wrap the endpoint selector on narrow terminals ([aff6bab](https://github.com/scriptology/open-course-cli/commit/aff6babad256212bcf475f2eb7a216ec1eef58c4))
* **ui:** word-wrap the report page instead of breaking words mid-line ([5e5bf6c](https://github.com/scriptology/open-course-cli/commit/5e5bf6c7dcb666628109c46ea70578d206d16d35))

## [0.7.0](https://github.com/scriptology/open-course-cli/compare/v0.6.1...v0.7.0) (2026-07-29)


### Features

* **ui:** localize all UI texts ([8289e19](https://github.com/scriptology/open-course-cli/commit/8289e19c4adec3a550191170198d09c16202a392))
* **ui:** localize all UI texts ([29b73ae](https://github.com/scriptology/open-course-cli/commit/29b73aeb6ddfa1ba901243244094bfe86e3c4ba3))


### Bug Fixes

* **ui:** print the analysis report to the main screen for native scroll and selection ([1b49abb](https://github.com/scriptology/open-course-cli/commit/1b49abbe89f5b2dcf492c0fc4b942a91e58c87bd))
* **ui:** print the analysis report to the main screen for native scroll and selection ([c0053d0](https://github.com/scriptology/open-course-cli/commit/c0053d0b5de7806ae23f50880d9d330e354e332a))
* **ui:** wrap long lines in topic documentation view ([122170c](https://github.com/scriptology/open-course-cli/commit/122170c961a2ac91454807f5010f36c55914ae9d))
* **ui:** wrap long lines in topic documentation view ([bf1a1ca](https://github.com/scriptology/open-course-cli/commit/bf1a1ca139cc2b5e17d7343d11e26d009248aa42))

## [0.6.1](https://github.com/scriptology/open-course-cli/compare/v0.6.0...v0.6.1) (2026-07-28)


### Bug Fixes

* graduate learning items by error association instead of text occurrence ([438234a](https://github.com/scriptology/open-course-cli/commit/438234ab1780845f20aea4d5a3001a9a8d5285c3))
* graduate learning items by error association instead of text occurrence ([50202ed](https://github.com/scriptology/open-course-cli/commit/50202edd9fb2b52db30d440848f88ebf35cefb6f))
* **ui:** capture the mouse only when content overflows so text stays selectable ([bbe5997](https://github.com/scriptology/open-course-cli/commit/bbe59971fb16087dbf51730fff61d948d1d7baa5))
* **ui:** capture the mouse only when content overflows so text stays selectable ([ac59f28](https://github.com/scriptology/open-course-cli/commit/ac59f28f5ec417a6a93386cf0234c15e5731d3c5))

## [0.6.0](https://github.com/scriptology/open-course-cli/compare/v0.5.3...v0.6.0) (2026-07-27)


### Features

* **update:** rename binary to opencourse, add update command and reliable update checks ([d6eaecb](https://github.com/scriptology/open-course-cli/commit/d6eaecb318d5761f2c913082496d164d05848aaa))
* **update:** rename binary to opencourse, add update command and reliable update checks ([27b168d](https://github.com/scriptology/open-course-cli/commit/27b168d0398c983b306c494c334d6c1f7c7d38de))


### Bug Fixes

* **ui:** show update notice under the version instead of a u-key prompt ([14df081](https://github.com/scriptology/open-course-cli/commit/14df081c4e70f340d5a0ee8eed1c2160f67ce65a))

## [0.5.3](https://github.com/scriptology/open-course-cli/compare/v0.5.2...v0.5.3) (2026-07-26)


### Bug Fixes

* store data globally in ~/.open-course-cli ([833d0fa](https://github.com/scriptology/open-course-cli/commit/833d0faa1538e18d0da674a1be2faf6fc918724c))
* store data globally in ~/.open-course-cli ([9904d4d](https://github.com/scriptology/open-course-cli/commit/9904d4decc1c9fd78ce70ec5c2febb154fda6266))
* **ui:** green selected options in selectors, onboarding logo margins ([44f7b6a](https://github.com/scriptology/open-course-cli/commit/44f7b6abb57d6d9fa1afcda6436bdd483abff2b5))
* **ui:** green selected options in selectors, onboarding logo margins ([2cc24c4](https://github.com/scriptology/open-course-cli/commit/2cc24c40fdf8d0cc4cdb71cad3597a9ad1351b52))
* **ui:** mark active pair with a gray localized tag ([c9090fb](https://github.com/scriptology/open-course-cli/commit/c9090fb24ac1156133ceca50a83b6bded8cfc4dd))
* **ui:** place active pair tag right after the label ([cddea47](https://github.com/scriptology/open-course-cli/commit/cddea47de96b4d3febfee90f387244d89b59ba1b))

## [0.5.2](https://github.com/scriptology/open-course-cli/compare/v0.5.1...v0.5.2) (2026-07-26)


### Bug Fixes

* store data globally in ~/.open-course-cli ([833d0fa](https://github.com/scriptology/open-course-cli/commit/833d0faa1538e18d0da674a1be2faf6fc918724c))
* store data globally in ~/.open-course-cli ([9904d4d](https://github.com/scriptology/open-course-cli/commit/9904d4decc1c9fd78ce70ec5c2febb154fda6266))

## [0.5.1](https://github.com/scriptology/open-course-cli/compare/v0.5.0...v0.5.1) (2026-07-19)


### Bug Fixes

* **ci:** chain release build to release-please via workflow_call ([#11](https://github.com/scriptology/open-course-cli/issues/11)) ([17657e3](https://github.com/scriptology/open-course-cli/commit/17657e3ea264e547cc88898a54df81df739ce6fd))

## [0.5.0](https://github.com/scriptology/open-course-cli/compare/v0.4.0...v0.5.0) (2026-07-19)


### Features

* **ui:** add global help overlay, error toasts, and consistent footers ([#7](https://github.com/scriptology/open-course-cli/issues/7)) ([e94777e](https://github.com/scriptology/open-course-cli/commit/e94777ef45a708cdc3f93a18cd071900a63cfdeb))

## [0.4.0](https://github.com/scriptology/open-course-cli/compare/v0.3.0...v0.4.0) (2026-07-19)


### Features

* remove formal/official style bias from prompts and curriculum domains ([1041a02](https://github.com/scriptology/open-course-cli/commit/1041a0259fc4d5531d1898efbea33513c3842e33))


### Bug Fixes

* **llm:** resolve gemini provider setup issues ([#1](https://github.com/scriptology/open-course-cli/issues/1)) ([47f5715](https://github.com/scriptology/open-course-cli/commit/47f57156aeb3f14b3f513b862bbd55982dac5860))
* terminal cleanup CPR leak + accept Enter on empty curriculum ([#4](https://github.com/scriptology/open-course-cli/issues/4)) ([278fa0c](https://github.com/scriptology/open-course-cli/commit/278fa0c93b33bf82370d6cebdcf7b7d635cf9684))
