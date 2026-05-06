<a name="1.5.0"></a>
## 1.5.0 (2026-05-06)


#### Bug Fixes

*   typo in log about creating session ([4e48d3bb](4e48d3bb))
*   configure circleci to push feature branches to dockerhub. ([86d5749c](86d5749c))
*   correct docs and travis for spake2 (#6); r=rfk ([5a1b8474](5a1b8474))

#### Features

*   add `--default_lang` / `PAIR_DEFAULT_LANG` to set default language ([628a8fe0](628a8fe0), closes [#83](83))
*   Add cargo audit ([b23c507c](b23c507c), closes [#75](75))
*   Use GCP header for backup localization info ([f0777722](f0777722), closes [#59](59))
*   Add security headers (#58) ([8a70fff1](8a70fff1))
*   Remove extraneous RefCells ([ba69d7de](ba69d7de), closes [#54](54))
*   Use python for integration testing. (#56) ([53ba8217](53ba8217), closes [#43](43))
*   Switch channelIDs to base64 (#48) ([ff3b1ece](ff3b1ece), closes [#47](47))
*   Use MozLogging, re-enable metrics (#46) ([c38039da](c38039da))
*   allow multiple connections to the channel server (#35) ([aca55ef4](aca55ef4), closes [#33](33))
*   add simple heartbeat and connection expiry (#31) ([9e0db68b](9e0db68b), closes [#24](24))
*   add location and metadata to message exchange (#14) ([39047102](39047102), closes [#8](8))
*   websocket channel server (#7) ([ef3ce02b](ef3ce02b))
*   initial linkserver prototype ([7abd0f47](7abd0f47), closes [#3](3))
