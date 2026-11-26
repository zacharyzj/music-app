# Reference and dependency notices

The application is a new Rust/Slint implementation. YesPlayMusic 3.0 (reference commit de9997d, GPL-3.0) was consulted for behavior and interaction structure; no Flutter code, logos, fonts, or screenshots are bundled.

The JavaScript NetEase plugin includes MIT-licensed source from ypm-plugin-netease, commit 1562291, and NeteaseCloudMusicApiEnhanced/api-enhanced, commit 23d9e9d. Their MIT notices are preserved below. No account permissions or copyright restrictions are bypassed; trial audio is rejected.

## MIT notices

Copyright (c) 2026 lymzzyh

Copyright (c) 2013-2022 Binaryify

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

## Native player and dependencies

The Windows build script downloads shinchiro/mpv-winbuild-cmake release 20261111,
mpv revision bcde6b93cc, x86_64 shared development build. Archive SHA-256:
273C079B11EA5575822F972756F4C9A71149A5C10130353ABEB8586738938452.

Build recipes: https://github.com/shinchiro/mpv-winbuild-cmake/tree/20261111

mpv source and licensing: https://github.com/mpv-player/mpv

Slint source and licensing: https://github.com/slint-ui/slint/tree/v1.19.1

rquickjs source and licensing: https://github.com/DelSkayn/rquickjs

QuickJS-NG source and licensing: https://github.com/quickjs-ng/quickjs

The NetEase JS package includes its original LICENSE and THIRD_PARTY_NOTICES.md. Its local changes cover the manifest (navigation, version, and media permissions) and cloud cover mapping: numeric image IDs fall back to the album image URL.

Rust dependencies and exact versions are recorded in Cargo.lock; each dependency retains its upstream license. Public redistribution needs the corresponding source/license materials for the selected Slint and libmpv distribution. This repository does not choose a commercial Slint license for the user.

## Audius plugin

The optional Audius plugin uses character mappings derived from OpenCC 1.1.9.
[OpenCC attribution](plugins/audius-js/THIRD_PARTY_NOTICES.md) and the
[Apache 2.0 license](plugins/audius-js/LICENSE.OpenCC) are included in its ZIP.
