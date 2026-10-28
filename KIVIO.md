# Kivio GPUI Kit

Fork of [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit) used by [Kivio](https://github.com/ZMGID/kivio). Branch `kivio` starts at upstream `v0.8.1`.

- `crates/`: gpui-kit 0.8.1, upstream layout.
- `gpui-pre/`: the crates.io `gpui-pre*` **0.3.10** packages, initially unpacked verbatim from their published `.crate` files and patched here (package names and versions unchanged).

Kivio consumes it through `[patch.crates-io]` pinned to a `rev` of this repo:

```toml
[patch.crates-io]
gpui-kit = { git = "https://github.com/ZMGID/kivio-gpui-kit", rev = "<rev>" }
gpui-pre = { git = "https://github.com/ZMGID/kivio-gpui-kit", rev = "<rev>" }
# …one line per package listed below
```

## Patches

Maintain each conceptual patch as a separate commit with regression coverage. Where noted, code is adapted from Zeron's Apache-2.0 GPUI fork [zeronsh/zui](https://github.com/zeronsh/zui) at `b564a34413bb7ef1da04121c4a287758f94993d9`. On a gpui-kit upgrade, rebase these commits and drop any that upstream now covers.

| Patch | Packages | Reference | Commit |
| --- | --- | --- | --- |
| Sidebar keyboard navigation | `gpui-component` | Kit Base `Button` focus handling and GPUI native keyboard clicks | `Fix sidebar keyboard navigation` |
| Image source eviction | `gpui-pre`, backend atlas tests | zui `crates/gpui/src/elements/img.rs`, `crates/gpui/src/app.rs` | `1b87590` |
| Scoped edge fade | `gpui-pre`, `gpui-pre-windows`, `gpui-pre-apple`, `gpui-pre-wgpu` | zui scene/window and DirectX/Metal/WGPU shaders; paths below | `0bd0646` |
| Backdrop blur | `gpui-pre`, `gpui-pre-windows`, `gpui-pre-apple`, `gpui-pre-wgpu` | zui scene/window and DirectX/Metal/WGPU renderers; paths below | `38e2968` |
| Verbatim selection copy | `gpui-base`, Kit regression | Existing `TextSelection::selected_text`; remove trimming in Root and TextView | `94ead70` |
| Windows tray hide/restore | `gpui-pre-windows` | Native `ShowWindowAsync`, preserving existing activation handling | `fix(windows): hide and restore windows for tray activation` |
| Frozen benchmark readout | `gpui-fps` | Existing frame sampler and trace guard; terminal publication for a bounded run | `fix(fps): freeze completed benchmark readings` |
| Opt-in frosted overlay surfaces | `gpui-component`, `gpui-pre` | Existing Kit dialog/menu/notification lifecycles and the backdrop API above | `feat(component): apply opt-in backdrop to overlay surfaces` |
| Deliberate held-button dragging | `gpui-pre` | zui `crates/gpui/src/elements/div.rs`, checked against local pointer lifecycles | `00012c5` |
| Rounded subtree clips and attachment media | `gpui-pre`, three native renderers, `gpui-component` | zui scene/window/shaders; Zeron `crates/ui/src/rounded_clip.rs` (MIT) | `9cc091c` |
| Fractional path coverage and clipped-path batching | `gpui-pre`, three native renderers | Local GPU review and real-pixel regressions | `b78185d` |
| Image alpha masks | `gpui-pre`, three native renderers | zui scene/window/shaders; Zeron `crates/ui/src/new_thread_background_mask.rs` | `0ce0a5e` |
| Shared-edge path coverage and once-only resolve copies | `gpui-pre`, three native renderers | Existing `Window::paint_layer` geometry contract and real-pixel regressions | `fix(render): preserve shared-edge coverage and path opacity` |
| Media borders outside descendant clips | `gpui-component`, Kit rendering tests | Native `rounded_clip`, existing `Div` paint and layout contracts | `fix(component): preserve rounded media borders` |

The sidebar patch starts from fork commit `e485b337f2ecb778cf3a5265488baf4dfaac663d`. It uses the existing `gpui-pre` 0.3.10 focus and click behavior, not code copied from Zeron. In the Kivio P1 Windows repro, Tab from the last gallery button skipped all sidebar destinations and wrapped to the first gallery button. The component regression tests exercise Tab, Enter and Space, disabled rows, retained focus, mouse clicks and submenu/caret behavior with `--features test-support`.

### P2 rendering and image lifecycle

- **Image eviction.** `ImageSource::evict(window, cx)` removes a `Resource` or `Image` from GPUI's default asset cache and drops the decoded image's atlas tiles in every window. Pass the currently updating window because it is temporarily absent from the app's window collection. A `Render` source drops its tiles without removing an asset; custom loaders and independently managed image caches remain the caller's responsibility. Eviction cancels pending default-cache loads without notifying subscribers or forcing a redraw; a later request starts a fresh load. The adaptation uses zui `crates/gpui/src/elements/img.rs` and `crates/gpui/src/app.rs`. Existing `CachedLoad` cancellation and stale-completion protections are retained. Regressions cover completed, failed and pending loads, replacement races, all animation frames, current/other windows, custom sources and GPU atlas re-upload.
- **Edge fade.** `Window::with_edge_fade` scopes a four-edge fade in window coordinates. Active nested scopes temporarily replace and then restore the outer scope; `None` or an inactive fade leaves it unchanged. Bands are at least one logical pixel. Quads, glyphs and sprites evaluate the squared ramp per fragment; paths and shadows use conservative primitive opacity, and decorations use their origin. Adapted from zui `crates/gpui/src/{window.rs,scene.rs}`, `crates/gpui_windows/src/shaders.hlsl`, `crates/gpui_macos/src/shaders.metal`, and `crates/gpui_wgpu/src/{shaders.wgsl,shaders_subpixel.wgsl}`; the WebGL decoder was adapted for the local WGPU layout. P2.5 subsequently adds the clip and image-mask records described below. Regressions exercise nested scopes, DPI scaling, primitive and shader ABI layouts, and backend GPU pixels.
  Cached views compare prepaint and paint-time fade scopes. When a paint-only scope changes, affected views are redrawn before the provisional frame is presented; unchanged scopes retain cached paint.
  The profiler records all retry passes as one draw with accumulated phase times and invalidations. Focus events, text-input configuration and accessibility updates are published only for the final pass.
- **Backdrop blur.** `Window::paint_backdrop_blur(bounds, corners, radius)` composites a Gaussian-filtered snapshot of already-painted **window content**, inside the current rectangular content mask and rounded bounds; it does not blur the desktop behind a transparent window. Positive radius is a logical-pixel sigma, clamped after scaling to 1–64 device pixels. Invalid or nonpositive radii and invalid or empty geometry produce no scene operation. Scene ordering uses the padded sampling footprint and actual shadow geometry, and treats blur as a barrier even inside layers, so earlier sampled content and later sharp foreground retain their order. DirectX uses two GPU blur passes; Metal uses MPS Gaussian blur; WGPU renders to a copyable intermediate before GPU filtering and compositing, including on surfaces without copy-source support. DirectX and WGPU filter native pixels before reducing each axis and retain bounded scratch sets per downsample factor. Alpha, resize, device/resource lifetime and multiple blurs are handled by the renderers. This is distinct from the existing OS-level `WindowBackgroundAppearance::Blurred`.

  The common scene and window code adapts zui `crates/gpui/src/{scene.rs,window.rs}`. DirectX adapts `crates/gpui_windows/{build.rs,src/directx_renderer.rs,src/directx_renderer/backdrop.rs,src/directx_renderer/backdrop_tests.rs,src/shaders.hlsl}`; Metal adapts `crates/gpui_macos/{build.rs,src/metal_renderer.rs,src/shaders.metal}`; WGPU adapts `crates/gpui_wgpu/src/{wgpu_renderer.rs,blur_kernel.rs,shaders.wgsl}`. `crates/kit/examples/backdrop_blur.rs` is adapted from zui `crates/gpui/examples/backdrop_blur.rs`. Scene regressions cover batch boundaries, nested layers, replay, clipping, invalid inputs and instance layout; backend regressions cover GPU pixels, ordering, alpha and resource lifetimes.
  The WGPU path-composition regression also adapts zui `crates/gpui_windows/src/directx_renderer/backdrop_tests.rs`.

`FpsMonitor::finish` drains pending samples, publishes once, releases its clock and trace guard, and freezes later renders. Start another run with a new monitor. All 35 FPS tests passed, including late frames, repeated finish, retained resource readings and trace ownership. Kivio's native Windows release run streamed 3,000 Unicode scalars while scrolling 2,000 messages for 60.007 seconds; the frozen HUD reported P95 3.6 ms and 0.0% dropped frames.

`OverlayBackdrop` is an optional application global: dialog, menu and notification surfaces blur their actual rounded bounds immediately before painting the theme tint and sharp contents. Without the global (or with `gpui-fast`) the existing surface is unchanged. Inherited opacity is respected; because the blur primitive has no alpha, fades taper sigma rather than blending a separate blurred image. Application policy stays outside Kit. All 734 component unit tests and 50 integration tests passed, including theme tint, scene order, zero/inherited opacity, stacked-dialog Escape and focus behavior.

The fork root `[patch.crates-io]` points its six vendored packages to local paths so workspace examples and tests exercise these changes rather than the published snapshots. Kivio's own workspace still supplies its pinned git patches. The example can be launched from the fork root with `cargo run -p gpui-kit --example backdrop_blur`; click its heading to compare blur and fade on and off. It shows nested blurred panels, sharp foreground and a four-edge fade.

Windows verification: GPUI core with `test-support,profiler`: 491 passed, 2 ignored; DirectX WARP: 39 passed, including real pixel readback; Kit verbatim clipboard regression: 1 passed. The Kit offscreen rendering executable explicitly skips Windows rather than substituting a fake renderer. Native release verification compared the backdrop example's effects on/off: nested panels blur their backgrounds while foreground stays sharp, and the four-edge fade disappears when disabled. Kivio's gallery verified light/dark frosted notifications, dialogs and menus, keyboard-origin focus restoration, and Escape dismissing only the top nested layer. Kivio Dev also verified close-to-tray and a second launch restoring the same primary HWND without starting another helper. The macOS gpui-pre WebView compile/lint job passed in CI run `40617546382`; macOS pixel behavior is not hardware-verified. Standalone `gpui-pre/*` manifests are excluded from the fork workspace and need an explicit `gpui-pre` path patch when testing a backend directly.

Vendored packages: `gpui-pre`, `gpui-pre-platform`, `gpui-pre-windows`, `gpui-pre-macos`, `gpui-pre-apple`, `gpui-pre-wgpu`.

Test prerequisites (`e658617`) restore the published core tests without the unpublished `util_macros::perf` dependency, use the existing IBM Plex Sans fixture, and restore `assets/fonts/lilex/Lilex-Regular.ttf` from the pinned zui reference above. Lilex is test-only and retains its accompanying SIL Open Font License in `assets/fonts/lilex/OFL.txt`; it is not part of the application's font bundle.

The WGPU font-family regression uses the existing full and subset IBM Plex Sans faces plus Lilex Regular; it does not require an unavailable Lilex Bold file. Its standalone Windows run passed 45 tests (font, atlas, shader and layout checks); Linux-only WGPU pixel tests are not included in that result.

CI compatibility fixes use the nondeprecated atomic `try_update` API (also compiled and exercised on Kivio's Rust 1.96), comma-separated Objective-C selectors, and no unused blur field on `gpui-fast`. Strict `-D warnings` verification passed 69 profiler tests and the fast component check. The typo vocabulary preserves native ABI names rather than renaming them; the renderer's actual prose typo is corrected.

Clipboard projections retain selected leading/trailing whitespace, blank lines and whitespace-only participants. Existing selection, reflow and copy-before-paint regressions now assert that exact projection rather than trimming it. The real drag/shortcut regression includes padded and whitespace-only plain text. Windows verification: all 1,490 base tests passed; all 425 Kit tests passed with native GPUI and again with `gpui-fast`. The full CI matrix is rerun after this clipboard contract migration.

The standalone Metal regression suite requires CoreVideo's `CVBase` feature for pixel-buffer lock flags and lock/unlock APIs. Enable it explicitly on the existing `objc2-core-video` dependency; the full workspace build did not compile these backend-local tests.

### P2.5 clips, image masks and dragging

- `Window::with_rounded_clip(bounds, corners, f)` applies device-space per-fragment coverage to quads, glyphs, images, paths, shadows, underlines, backdrop blur and Apple's supported surfaces. Nested scopes use the innermost rounded outline while retaining the intersection of ancestor rectangular masks; they do not intersect arbitrary rounded outlines. Hit testing stays rectangular and deferred draws have their own scope. Invalid or empty clips reject drawing, including at fractional DPI. Cached views compare both prepaint and paint-time clip/mask state and reuse the existing redraw-before-present mechanism.
- Paths apply rounded coverage after MSAA resolve. Same-order, same-clip paths share a resolve under `Window::paint_layer`'s non-overlapping-geometry contract, preserving complementary edge samples. Different clips or overlapping draw orders remain isolated; contiguous unclipped paths retain batching. Overlapping copy rectangles use one union copy so transparent pixels are not composited twice. The conservative device-pixel rectangle keeps fractional AA edges from being cut off before shading; no general offscreen layer or new texture per element is introduced.
- Native `gpui_component::rounded_clip` wraps a child without changing its layout; its MIT notice credits Zeron `3d2f1e64af3b40ec1354ca199fbe439c666f3681`. `AttachmentMedia` paints its rounded background and border once, outside the clip used by images, custom children and overlays. The outer card's focus, shadow and remove control also stay outside the clip. The separate `gpui-fast` path retains its prior behavior.
- `Window::paint_image` takes a final `Option<ImageAlphaMask>`; existing callers explicitly pass `None`. `ImageAlphaMask::new(bounds, radius, feather)` excludes a rounded window-space rectangle, with optional clearance and bottom fade. Nonpositive feather disables the mask; exclusion and bottom fade combine with `min`, then image alpha, edge fade and clip coverage are applied once. Moving or resizing the mask does not change the source image, atlas key or uploaded tile. Shader records and Metal's generated-header mirror change together.
- Drag promotion requires a held left button and movement strictly greater than four logical pixels from the press, including diagonal distance. A mismatched or missing pressed button clears stale press state; right-button motion does not consume the left-drag listener. Text-selection thresholds and drag cursor-offset semantics are unchanged.

Sources are the pinned zui scene/window and DirectX/Metal/WGPU shader files above, Zeron's two UI helpers, and the actual Kit 0.8.1 / gpui-pre 0.3.10 dependency sources. Zeron's distributed component fork is 0.5.2, not the selected 0.8.1 Kit; its APIs were not copied unchecked.

Windows verification: core 501 passed (2 existing ignores), DirectX WARP 48 passed, WGPU 72 passed, component 735 unit and 50 integration tests passed; strict workspace Clippy passed with the CI exclusions. The renderer suites read real GPU pixels, including fractional path edges, clip/opacity combinations and image-mask composition; WGPU exercises both storage-buffer and texture-backed instance decoding on Windows. Core regressions cover clip caching and held-button drag transitions; image regressions check DPI, cropping and atlas reuse. Metal is checked by the macOS CI job, not a Windows substitute. Actual browser WebGL and macOS window interaction remain separate platform checks; WGPU's pre-existing unsupported surface path is not expanded here.

Review follow-up verification on Windows: DirectX WARP 52 passed, WGPU 75 passed, GPUI core 501 passed with 2 existing ignores. Pixel regressions cover the complementary-triangle seam, half-alpha paths with overlapping bounds, existing clip isolation, and both WGPU instance transports. These local results do not claim Metal or browser WebGL execution.

Media-border review verification: component 738 unit and 50 integration tests passed on Windows. Scene regressions compare ordinary `Div` and media paint, asymmetric radii/borders, opacity, descendant clipping, and failed-image retry interaction at fractional or doubled scale. New real-pixel attachment cases live in `crates/kit/tests/rendering.rs`; that harness runs on Metal/Linux WGPU, not Windows.

Licensed under Apache-2.0, same as upstream (see `LICENSE-APACHE`).
