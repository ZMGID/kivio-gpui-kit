# Kivio GPUI Kit

Fork of [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit) used by [Kivio](https://github.com/ZMGID/kivio). Branch `kivio` starts at upstream `v0.8.1`.

- `crates/`: gpui-kit 0.8.1, upstream layout.
- `gpui-pre/`: the crates.io `gpui-pre*` **0.3.10** packages that Kivio patches, unpacked verbatim from their published `.crate` files (package names and versions unchanged).

Kivio consumes it through `[patch.crates-io]` pinned to a `rev` of this repo:

```toml
[patch.crates-io]
gpui-kit = { git = "https://github.com/ZMGID/kivio-gpui-kit", rev = "<rev>" }
gpui-pre = { git = "https://github.com/ZMGID/kivio-gpui-kit", rev = "<rev>" }
# …one line per package listed below
```

## Patches

Each patch is one commit with its own regression test. Implementations follow Zeron's GPUI fork ([zeronsh/zui](https://github.com/zeronsh/zui) `b564a344`, Apache-2.0) where noted. On a gpui-kit upgrade, rebase these commits and drop any that upstream now covers.

| Patch | Packages | Reference | Commit |
| --- | --- | --- | --- |
| _(none yet)_ | | | |

Vendored packages: `gpui-pre`, `gpui-pre-platform`, `gpui-pre-windows`, `gpui-pre-macos`, `gpui-pre-apple`, `gpui-pre-wgpu`.

Licensed under Apache-2.0, same as upstream (see `LICENSE-APACHE`).
