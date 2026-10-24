# 第三方声明

本项目自身的 MIT 许可证不替代第三方组件的许可证。

前端包含 Marked 15.0.12，见 [Marked 许可证](third-party/marked-LICENSE.txt)；原文件保留版权头。

`dashboard-tauri/dist/app.js` 中的警告、检查、收件箱和加号圆圈内联图标沿用 Lucide / Feather 的几何路径，调整了笔画、尺寸及部分 SVG 写法。随附 [Lucide ISC / MIT 声明](third-party/lucide-LICENSE.txt) 和 [Feather MIT 声明](third-party/feather-LICENSE.txt)，构建时也嵌入 App。许可证副本分别取自 [Lucide 500620a](https://github.com/lucide-icons/lucide/blob/500620a2e8123f8d1db191538886dc0c223f69a9/LICENSE) 与 [Feather 3dc050d](https://github.com/feathericons/feather/blob/3dc050d97405062eba78aa57115c0a15c63abdaa/LICENSE)。App 启动图标从仓库中的 SVG 生成，不包含设计预览所用的 Bootstrap 字体。

内嵌 SQLite 3.53.2 使用上游公开领域声明，见 [SQLite 声明](third-party/sqlite-PUBLIC-DOMAIN.txt) 与 [上游说明](https://sqlite.org/copyright.html)。Zstandard 1.5.7 选择 BSD-3-Clause，见 [Zstandard 许可证](third-party/zstd-LICENSE.txt)。

Windows WebView2 Rust 封装的缺失许可证正文取自上游 [b74dc5e](https://github.com/wravery/webview2-rs/blob/b74dc5e2b394044bea5191052868ce7a106c202c/LICENSE) 与 [dffa41a](https://github.com/wravery/webview2-rs/blob/dffa41a8a46d3f5565eefbff2de57d38d399f158/LICENSE)，两个版本内容相同；见 [许可证副本](third-party/webview2-rs-LICENSE.txt)。

下表根据 Cargo.lock 与 macOS ARM64 / Windows x64 依赖图的并集生成，涵盖构建依赖及传递依赖，不代表每个组件都被链接进最终二进制。每项源码可从对应版本的 crates.io 页面取得；构建脚本收集当前平台的许可证正文并附入 App，缺失正文时停止。MPL 组件的源代码取得方式见对应链接，未修改依赖源码。

| 组件 | 版本 | 许可证表达式 | 平台 | 源码 |
|---|---|---|---|---|
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/adler2/2.0.1) |
| ahash | 0.8.12 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/ahash/0.8.12) |
| aho-corasick | 1.1.4 | Unlicense OR MIT | macos / windows | [版本与源码](https://crates.io/crates/aho-corasick/1.1.4) |
| alloc-no-stdlib | 2.0.4 | BSD-3-Clause | macos / windows | [版本与源码](https://crates.io/crates/alloc-no-stdlib/2.0.4) |
| alloc-stdlib | 0.2.4 | BSD-3-Clause | macos / windows | [版本与源码](https://crates.io/crates/alloc-stdlib/0.2.4) |
| anyhow | 1.0.103 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/anyhow/1.0.103) |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/autocfg/1.5.1) |
| base64 | 0.21.7 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/base64/0.21.7) |
| base64 | 0.22.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/base64/0.22.1) |
| bit-set | 0.8.0 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/bit-set/0.8.0) |
| bit-vec | 0.8.0 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/bit-vec/0.8.0) |
| bitflags | 1.3.2 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/bitflags/1.3.2) |
| bitflags | 2.13.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/bitflags/2.13.0) |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/block-buffer/0.10.4) |
| block2 | 0.6.2 | MIT | macos | [版本与源码](https://crates.io/crates/block2/0.6.2) |
| borrow-or-share | 0.2.4 | MIT-0 | macos / windows | [版本与源码](https://crates.io/crates/borrow-or-share/0.2.4) |
| brotli | 8.0.4 | BSD-3-Clause AND MIT | macos / windows | [版本与源码](https://crates.io/crates/brotli/8.0.4) |
| brotli-decompressor | 5.0.3 | BSD-3-Clause/MIT | macos / windows | [版本与源码](https://crates.io/crates/brotli-decompressor/5.0.3) |
| bs58 | 0.5.1 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/bs58/0.5.1) |
| bytecount | 0.6.9 | Apache-2.0/MIT | macos / windows | [版本与源码](https://crates.io/crates/bytecount/0.6.9) |
| byteorder | 1.5.0 | Unlicense OR MIT | macos / windows | [版本与源码](https://crates.io/crates/byteorder/1.5.0) |
| bytes | 1.12.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/bytes/1.12.0) |
| camino | 1.2.4 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/camino/1.2.4) |
| cargo-platform | 0.1.9 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/cargo-platform/0.1.9) |
| cargo_metadata | 0.19.2 | MIT | macos / windows | [版本与源码](https://crates.io/crates/cargo_metadata/0.19.2) |
| cargo_toml | 0.22.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/cargo_toml/0.22.3) |
| cc | 1.2.66 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/cc/1.2.66) |
| cfb | 0.7.3 | MIT | macos / windows | [版本与源码](https://crates.io/crates/cfb/0.7.3) |
| cfg-if | 1.0.4 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/cfg-if/1.0.4) |
| chrono | 0.4.45 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/chrono/0.4.45) |
| cookie | 0.18.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/cookie/0.18.1) |
| core-foundation | 0.10.1 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/core-foundation/0.10.1) |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/core-foundation-sys/0.8.7) |
| core-graphics | 0.25.0 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/core-graphics/0.25.0) |
| core-graphics-types | 0.2.0 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/core-graphics-types/0.2.0) |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/cpufeatures/0.2.17) |
| crc32fast | 1.5.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/crc32fast/1.5.0) |
| crossbeam-channel | 0.5.16 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/crossbeam-channel/0.5.16) |
| crossbeam-utils | 0.8.22 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/crossbeam-utils/0.8.22) |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/crypto-common/0.1.7) |
| cssparser | 0.36.0 | MPL-2.0 | macos / windows | [版本与源码](https://crates.io/crates/cssparser/0.36.0) |
| cssparser-macros | 0.6.1 | MPL-2.0 | macos / windows | [版本与源码](https://crates.io/crates/cssparser-macros/0.6.1) |
| ctor | 0.8.0 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/ctor/0.8.0) |
| ctor-proc-macro | 0.0.7 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/ctor-proc-macro/0.0.7) |
| darling | 0.23.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/darling/0.23.0) |
| darling_core | 0.23.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/darling_core/0.23.0) |
| darling_macro | 0.23.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/darling_macro/0.23.0) |
| deranged | 0.5.8 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/deranged/0.5.8) |
| derive_more | 2.1.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/derive_more/2.1.1) |
| derive_more-impl | 2.1.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/derive_more-impl/2.1.1) |
| digest | 0.10.7 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/digest/0.10.7) |
| dirs | 5.0.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/dirs/5.0.1) |
| dirs | 6.0.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/dirs/6.0.0) |
| dirs-sys | 0.4.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/dirs-sys/0.4.1) |
| dirs-sys | 0.5.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/dirs-sys/0.5.0) |
| dispatch2 | 0.3.1 | Zlib OR Apache-2.0 OR MIT | macos | [版本与源码](https://crates.io/crates/dispatch2/0.3.1) |
| displaydoc | 0.2.6 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/displaydoc/0.2.6) |
| dom_query | 0.27.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/dom_query/0.27.0) |
| dpi | 0.1.2 | Apache-2.0 AND MIT | macos / windows | [版本与源码](https://crates.io/crates/dpi/0.1.2) |
| dtoa | 1.0.11 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/dtoa/1.0.11) |
| dtoa-short | 0.3.5 | MPL-2.0 | macos / windows | [版本与源码](https://crates.io/crates/dtoa-short/0.3.5) |
| dtor | 0.3.0 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/dtor/0.3.0) |
| dtor-proc-macro | 0.0.6 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/dtor-proc-macro/0.0.6) |
| dunce | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/dunce/1.0.5) |
| dyn-clone | 1.0.20 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/dyn-clone/1.0.20) |
| email_address | 0.2.9 | MIT | macos / windows | [版本与源码](https://crates.io/crates/email_address/0.2.9) |
| embed-resource | 3.0.11 | MIT | macos / windows | [版本与源码](https://crates.io/crates/embed-resource/3.0.11) |
| embed_plist | 1.2.2 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/embed_plist/1.2.2) |
| equivalent | 1.0.2 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/equivalent/1.0.2) |
| erased-serde | 0.4.10 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/erased-serde/0.4.10) |
| fallible-iterator | 0.3.0 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/fallible-iterator/0.3.0) |
| fallible-streaming-iterator | 0.1.9 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/fallible-streaming-iterator/0.1.9) |
| fancy-regex | 0.14.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/fancy-regex/0.14.0) |
| fastrand | 2.4.1 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/fastrand/2.4.1) |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/fdeflate/0.3.7) |
| find-msvc-tools | 0.1.9 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/find-msvc-tools/0.1.9) |
| flate2 | 1.1.9 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/flate2/1.1.9) |
| fluent-uri | 0.3.2 | MIT | macos / windows | [版本与源码](https://crates.io/crates/fluent-uri/0.3.2) |
| fnv | 1.0.7 | Apache-2.0 / MIT | macos / windows | [版本与源码](https://crates.io/crates/fnv/1.0.7) |
| foldhash | 0.2.0 | Zlib | macos / windows | [版本与源码](https://crates.io/crates/foldhash/0.2.0) |
| foreign-types | 0.5.0 | MIT/Apache-2.0 | macos | [版本与源码](https://crates.io/crates/foreign-types/0.5.0) |
| foreign-types-macros | 0.2.3 | MIT/Apache-2.0 | macos | [版本与源码](https://crates.io/crates/foreign-types-macros/0.2.3) |
| foreign-types-shared | 0.3.1 | MIT/Apache-2.0 | macos | [版本与源码](https://crates.io/crates/foreign-types-shared/0.3.1) |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/form_urlencoded/1.2.2) |
| fraction | 0.15.4 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/fraction/0.15.4) |
| generic-array | 0.14.7 | MIT | macos / windows | [版本与源码](https://crates.io/crates/generic-array/0.14.7) |
| getrandom | 0.3.4 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/getrandom/0.3.4) |
| getrandom | 0.4.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/getrandom/0.4.3) |
| glob | 0.3.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/glob/0.3.3) |
| hashbrown | 0.12.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/hashbrown/0.12.3) |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/hashbrown/0.17.1) |
| hashlink | 0.12.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/hashlink/0.12.2) |
| heck | 0.5.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/heck/0.5.0) |
| hex | 0.4.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/hex/0.4.3) |
| html5ever | 0.38.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/html5ever/0.38.0) |
| http | 1.4.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/http/1.4.2) |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/iana-time-zone/0.1.65) |
| ico | 0.5.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/ico/0.5.0) |
| icu_collections | 2.2.0 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/icu_collections/2.2.0) |
| icu_locale_core | 2.2.0 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/icu_locale_core/2.2.0) |
| icu_normalizer | 2.2.0 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/icu_normalizer/2.2.0) |
| icu_normalizer_data | 2.2.0 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/icu_normalizer_data/2.2.0) |
| icu_properties | 2.2.0 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/icu_properties/2.2.0) |
| icu_properties_data | 2.2.0 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/icu_properties_data/2.2.0) |
| icu_provider | 2.2.0 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/icu_provider/2.2.0) |
| ident_case | 1.0.1 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/ident_case/1.0.1) |
| idna | 1.1.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/idna/1.1.0) |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/idna_adapter/1.2.2) |
| indexmap | 1.9.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/indexmap/1.9.3) |
| indexmap | 2.14.0 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/indexmap/2.14.0) |
| infer | 0.19.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/infer/0.19.0) |
| itoa | 1.0.18 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/itoa/1.0.18) |
| jobserver | 0.1.35 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/jobserver/0.1.35) |
| json-patch | 3.0.1 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/json-patch/3.0.1) |
| jsonptr | 0.6.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/jsonptr/0.6.3) |
| jsonschema | 0.28.3 | MIT | macos / windows | [版本与源码](https://crates.io/crates/jsonschema/0.28.3) |
| keyboard-types | 0.7.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/keyboard-types/0.7.0) |
| lazy_static | 1.5.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/lazy_static/1.5.0) |
| libc | 0.2.186 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/libc/0.2.186) |
| libsqlite3-sys | 0.38.2 | MIT | macos / windows | [版本与源码](https://crates.io/crates/libsqlite3-sys/0.38.2) |
| litemap | 0.8.2 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/litemap/0.8.2) |
| lock_api | 0.4.14 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/lock_api/0.4.14) |
| log | 0.4.33 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/log/0.4.33) |
| markup5ever | 0.38.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/markup5ever/0.38.0) |
| memchr | 2.8.2 | Unlicense OR MIT | macos / windows | [版本与源码](https://crates.io/crates/memchr/2.8.2) |
| mime | 0.3.17 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/mime/0.3.17) |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/miniz_oxide/0.8.9) |
| mio | 1.2.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/mio/1.2.1) |
| muda | 0.19.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/muda/0.19.3) |
| new_debug_unreachable | 1.0.6 | MIT | macos / windows | [版本与源码](https://crates.io/crates/new_debug_unreachable/1.0.6) |
| num | 0.4.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num/0.4.3) |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num-bigint/0.4.8) |
| num-cmp | 0.1.0 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num-cmp/0.1.0) |
| num-complex | 0.4.6 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num-complex/0.4.6) |
| num-conv | 0.2.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num-conv/0.2.2) |
| num-integer | 0.1.46 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num-integer/0.1.46) |
| num-iter | 0.1.45 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num-iter/0.1.45) |
| num-rational | 0.4.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num-rational/0.4.2) |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/num-traits/0.2.19) |
| objc2 | 0.6.4 | MIT | macos | [版本与源码](https://crates.io/crates/objc2/0.6.4) |
| objc2-app-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | macos | [版本与源码](https://crates.io/crates/objc2-app-kit/0.3.2) |
| objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT | macos | [版本与源码](https://crates.io/crates/objc2-core-foundation/0.3.2) |
| objc2-core-graphics | 0.3.2 | Zlib OR Apache-2.0 OR MIT | macos | [版本与源码](https://crates.io/crates/objc2-core-graphics/0.3.2) |
| objc2-encode | 4.1.0 | MIT | macos | [版本与源码](https://crates.io/crates/objc2-encode/4.1.0) |
| objc2-exception-helper | 0.1.1 | Zlib OR Apache-2.0 OR MIT | macos | [版本与源码](https://crates.io/crates/objc2-exception-helper/0.1.1) |
| objc2-foundation | 0.3.2 | MIT | macos | [版本与源码](https://crates.io/crates/objc2-foundation/0.3.2) |
| objc2-io-surface | 0.3.2 | Zlib OR Apache-2.0 OR MIT | macos | [版本与源码](https://crates.io/crates/objc2-io-surface/0.3.2) |
| objc2-web-kit | 0.3.2 | Zlib OR Apache-2.0 OR MIT | macos | [版本与源码](https://crates.io/crates/objc2-web-kit/0.3.2) |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/once_cell/1.21.4) |
| option-ext | 0.2.0 | MPL-2.0 | macos / windows | [版本与源码](https://crates.io/crates/option-ext/0.2.0) |
| outref | 0.5.2 | MIT | macos / windows | [版本与源码](https://crates.io/crates/outref/0.5.2) |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/parking_lot/0.12.5) |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/parking_lot_core/0.9.12) |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/percent-encoding/2.3.2) |
| phf | 0.13.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/phf/0.13.1) |
| phf_codegen | 0.13.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/phf_codegen/0.13.1) |
| phf_generator | 0.13.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/phf_generator/0.13.1) |
| phf_macros | 0.13.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/phf_macros/0.13.1) |
| phf_shared | 0.13.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/phf_shared/0.13.1) |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/pin-project-lite/0.2.17) |
| pkg-config | 0.3.33 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/pkg-config/0.3.33) |
| plist | 1.10.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/plist/1.10.0) |
| png | 0.17.16 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/png/0.17.16) |
| png | 0.18.1 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/png/0.18.1) |
| potential_utf | 0.1.5 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/potential_utf/0.1.5) |
| powerfmt | 0.2.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/powerfmt/0.2.0) |
| precomputed-hash | 0.1.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/precomputed-hash/0.1.1) |
| proc-macro2 | 1.0.106 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/proc-macro2/1.0.106) |
| quick-xml | 0.41.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/quick-xml/0.41.0) |
| quote | 1.0.46 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/quote/1.0.46) |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | macos / windows | [版本与源码](https://crates.io/crates/raw-window-handle/0.6.2) |
| ref-cast | 1.0.25 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/ref-cast/1.0.25) |
| ref-cast-impl | 1.0.25 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/ref-cast-impl/1.0.25) |
| referencing | 0.28.3 | MIT | macos / windows | [版本与源码](https://crates.io/crates/referencing/0.28.3) |
| regex | 1.12.4 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/regex/1.12.4) |
| regex-automata | 0.4.14 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/regex-automata/0.4.14) |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/regex-syntax/0.8.11) |
| rusqlite | 0.40.2 | MIT | macos / windows | [版本与源码](https://crates.io/crates/rusqlite/0.40.2) |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/rustc-hash/2.1.3) |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/rustc_version/0.4.1) |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 | macos / windows | [版本与源码](https://crates.io/crates/ryu/1.0.23) |
| same-file | 1.0.6 | Unlicense/MIT | macos / windows | [版本与源码](https://crates.io/crates/same-file/1.0.6) |
| schemars | 0.8.22 | MIT | macos / windows | [版本与源码](https://crates.io/crates/schemars/0.8.22) |
| schemars | 0.9.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/schemars/0.9.0) |
| schemars | 1.2.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/schemars/1.2.1) |
| schemars_derive | 0.8.22 | MIT | macos / windows | [版本与源码](https://crates.io/crates/schemars_derive/0.8.22) |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/scopeguard/1.2.0) |
| selectors | 0.36.1 | MPL-2.0 | macos / windows | [版本与源码](https://crates.io/crates/selectors/0.36.1) |
| semver | 1.0.28 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/semver/1.0.28) |
| serde | 1.0.228 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde/1.0.228) |
| serde-untagged | 0.1.9 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde-untagged/0.1.9) |
| serde_core | 1.0.228 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_core/1.0.228) |
| serde_derive | 1.0.228 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_derive/1.0.228) |
| serde_derive_internals | 0.29.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_derive_internals/0.29.1) |
| serde_json | 1.0.150 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_json/1.0.150) |
| serde_repr | 0.1.20 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_repr/0.1.20) |
| serde_spanned | 1.1.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_spanned/1.1.1) |
| serde_with | 3.21.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_with/3.21.0) |
| serde_with_macros | 3.21.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_with_macros/3.21.0) |
| serde_yaml | 0.9.34+deprecated | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serde_yaml/0.9.34+deprecated) |
| serialize-to-javascript | 0.1.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serialize-to-javascript/0.1.2) |
| serialize-to-javascript-impl | 0.1.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/serialize-to-javascript-impl/0.1.2) |
| servo_arc | 0.4.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/servo_arc/0.4.3) |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/sha2/0.10.9) |
| shlex | 2.0.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/shlex/2.0.1) |
| simd-adler32 | 0.3.9 | MIT | macos / windows | [版本与源码](https://crates.io/crates/simd-adler32/0.3.9) |
| siphasher | 1.0.3 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/siphasher/1.0.3) |
| smallvec | 1.15.2 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/smallvec/1.15.2) |
| socket2 | 0.6.4 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/socket2/0.6.4) |
| softbuffer | 0.4.8 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/softbuffer/0.4.8) |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/stable_deref_trait/1.2.1) |
| string_cache | 0.9.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/string_cache/0.9.0) |
| string_cache_codegen | 0.6.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/string_cache_codegen/0.6.1) |
| strsim | 0.11.1 | MIT | macos / windows | [版本与源码](https://crates.io/crates/strsim/0.11.1) |
| swift-rs | 1.0.7 | MIT OR Apache-2.0 | macos | [版本与源码](https://crates.io/crates/swift-rs/1.0.7) |
| syn | 2.0.118 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/syn/2.0.118) |
| synstructure | 0.13.2 | MIT | macos / windows | [版本与源码](https://crates.io/crates/synstructure/0.13.2) |
| tao | 0.35.3 | Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/tao/0.35.3) |
| tauri | 2.11.5 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/tauri/2.11.5) |
| tauri-build | 2.6.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/tauri-build/2.6.3) |
| tauri-codegen | 2.6.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/tauri-codegen/2.6.3) |
| tauri-macros | 2.6.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/tauri-macros/2.6.3) |
| tauri-runtime | 2.11.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/tauri-runtime/2.11.3) |
| tauri-runtime-wry | 2.11.4 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/tauri-runtime-wry/2.11.4) |
| tauri-utils | 2.9.3 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/tauri-utils/2.9.3) |
| tauri-winres | 0.3.6 | MIT | macos / windows | [版本与源码](https://crates.io/crates/tauri-winres/0.3.6) |
| tendril | 0.5.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/tendril/0.5.1) |
| thiserror | 1.0.69 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/thiserror/1.0.69) |
| thiserror | 2.0.18 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/thiserror/2.0.18) |
| thiserror-impl | 1.0.69 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/thiserror-impl/1.0.69) |
| thiserror-impl | 2.0.18 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/thiserror-impl/2.0.18) |
| time | 0.3.53 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/time/0.3.53) |
| time-core | 0.1.9 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/time-core/0.1.9) |
| time-macros | 0.2.31 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/time-macros/0.2.31) |
| tinystr | 0.8.3 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/tinystr/0.8.3) |
| tinyvec | 1.11.0 | Zlib OR Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/tinyvec/1.11.0) |
| tinyvec_macros | 0.1.1 | MIT OR Apache-2.0 OR Zlib | macos / windows | [版本与源码](https://crates.io/crates/tinyvec_macros/0.1.1) |
| tokio | 1.52.3 | MIT | macos / windows | [版本与源码](https://crates.io/crates/tokio/1.52.3) |
| toml | 0.9.12+spec-1.1.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/toml/0.9.12+spec-1.1.0) |
| toml | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/toml/1.1.2+spec-1.1.0) |
| toml_datetime | 0.7.5+spec-1.1.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/toml_datetime/0.7.5+spec-1.1.0) |
| toml_datetime | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/toml_datetime/1.1.1+spec-1.1.0) |
| toml_parser | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/toml_parser/1.1.2+spec-1.1.0) |
| toml_writer | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/toml_writer/1.1.1+spec-1.1.0) |
| tracing | 0.1.44 | MIT | windows | [版本与源码](https://crates.io/crates/tracing/0.1.44) |
| tracing-core | 0.1.36 | MIT | windows | [版本与源码](https://crates.io/crates/tracing-core/0.1.36) |
| tray-icon | 0.24.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/tray-icon/0.24.1) |
| typeid | 1.0.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/typeid/1.0.3) |
| typenum | 1.20.1 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/typenum/1.20.1) |
| unic-char-property | 0.9.0 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/unic-char-property/0.9.0) |
| unic-char-range | 0.9.0 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/unic-char-range/0.9.0) |
| unic-common | 0.9.0 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/unic-common/0.9.0) |
| unic-ucd-ident | 0.9.0 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/unic-ucd-ident/0.9.0) |
| unic-ucd-version | 0.9.0 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/unic-ucd-version/0.9.0) |
| unicode-ident | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/unicode-ident/1.0.24) |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/unicode-segmentation/1.13.3) |
| unsafe-libyaml | 0.2.11 | MIT | macos / windows | [版本与源码](https://crates.io/crates/unsafe-libyaml/0.2.11) |
| url | 2.5.8 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/url/2.5.8) |
| urlpattern | 0.3.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/urlpattern/0.3.0) |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/utf8_iter/1.0.4) |
| uuid | 1.23.4 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/uuid/1.23.4) |
| uuid-simd | 0.8.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/uuid-simd/0.8.0) |
| vcpkg | 0.2.15 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/vcpkg/0.2.15) |
| version_check | 0.9.5 | MIT/Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/version_check/0.9.5) |
| vsimd | 0.8.0 | MIT | macos / windows | [版本与源码](https://crates.io/crates/vsimd/0.8.0) |
| vswhom | 0.1.0 | MIT | windows | [版本与源码](https://crates.io/crates/vswhom/0.1.0) |
| vswhom-sys | 0.1.3 | MIT | windows | [版本与源码](https://crates.io/crates/vswhom-sys/0.1.3) |
| walkdir | 2.5.0 | Unlicense/MIT | macos / windows | [版本与源码](https://crates.io/crates/walkdir/2.5.0) |
| web_atoms | 0.2.5 | MIT OR Apache-2.0 | macos / windows | [版本与源码](https://crates.io/crates/web_atoms/0.2.5) |
| webview2-com | 0.38.2 | MIT | windows | [版本与源码](https://crates.io/crates/webview2-com/0.38.2) |
| webview2-com-macros | 0.8.1 | MIT | windows | [版本与源码](https://crates.io/crates/webview2-com-macros/0.8.1) |
| webview2-com-sys | 0.38.2 | MIT | windows | [版本与源码](https://crates.io/crates/webview2-com-sys/0.38.2) |
| winapi-util | 0.1.11 | Unlicense OR MIT | windows | [版本与源码](https://crates.io/crates/winapi-util/0.1.11) |
| window-vibrancy | 0.6.0 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/window-vibrancy/0.6.0) |
| windows | 0.61.3 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows/0.61.3) |
| windows-collections | 0.2.0 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-collections/0.2.0) |
| windows-core | 0.61.2 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-core/0.61.2) |
| windows-future | 0.2.1 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-future/0.2.1) |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-implement/0.60.2) |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-interface/0.59.3) |
| windows-link | 0.1.3 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-link/0.1.3) |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-link/0.2.1) |
| windows-numerics | 0.2.0 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-numerics/0.2.0) |
| windows-result | 0.3.4 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-result/0.3.4) |
| windows-strings | 0.4.2 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-strings/0.4.2) |
| windows-sys | 0.48.0 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-sys/0.48.0) |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-sys/0.59.0) |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-sys/0.61.2) |
| windows-targets | 0.48.5 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-targets/0.48.5) |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-targets/0.52.6) |
| windows-threading | 0.1.0 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-threading/0.1.0) |
| windows-version | 0.1.7 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows-version/0.1.7) |
| windows_x86_64_msvc | 0.48.5 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows_x86_64_msvc/0.48.5) |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 | windows | [版本与源码](https://crates.io/crates/windows_x86_64_msvc/0.52.6) |
| winnow | 0.7.15 | MIT | macos / windows | [版本与源码](https://crates.io/crates/winnow/0.7.15) |
| winnow | 1.0.3 | MIT | macos / windows | [版本与源码](https://crates.io/crates/winnow/1.0.3) |
| winreg | 0.55.0 | MIT | windows | [版本与源码](https://crates.io/crates/winreg/0.55.0) |
| writeable | 0.6.3 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/writeable/0.6.3) |
| wry | 0.55.1 | Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/wry/0.55.1) |
| yoke | 0.8.3 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/yoke/0.8.3) |
| yoke-derive | 0.8.2 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/yoke-derive/0.8.2) |
| zerocopy | 0.8.52 | BSD-2-Clause OR Apache-2.0 OR MIT | macos / windows | [版本与源码](https://crates.io/crates/zerocopy/0.8.52) |
| zerofrom | 0.1.8 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/zerofrom/0.1.8) |
| zerofrom-derive | 0.1.7 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/zerofrom-derive/0.1.7) |
| zerotrie | 0.2.4 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/zerotrie/0.2.4) |
| zerovec | 0.11.6 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/zerovec/0.11.6) |
| zerovec-derive | 0.11.3 | Unicode-3.0 | macos / windows | [版本与源码](https://crates.io/crates/zerovec-derive/0.11.3) |
| zmij | 1.0.21 | MIT | macos / windows | [版本与源码](https://crates.io/crates/zmij/1.0.21) |
| zstd | 0.14.3 | BSD-3-Clause | macos / windows | [版本与源码](https://crates.io/crates/zstd/0.14.3) |
| zstd-safe | 8.2.0 | BSD-3-Clause | macos / windows | [版本与源码](https://crates.io/crates/zstd-safe/8.2.0) |
| zstd-sys | 2.1.2+zstd.1.5.7 | BSD-3-Clause | macos / windows | [版本与源码](https://crates.io/crates/zstd-sys/2.1.2+zstd.1.5.7) |
