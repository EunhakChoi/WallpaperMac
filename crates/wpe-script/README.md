# SceneScript reconstruction service

This crate executes ECMAScript modules in Boa 0.22.0 (pure Rust; MSRV 1.91). It does not load the Windows SceneScript DLL or a native wallpaper/UI process. The integration boundary is JSON scene state, typed frame input, patches, diagnostics, hook results and explicit host commands.

```rust,ignore
let mut vm = SceneRuntime::new(scene, modules, options)?;
vm.set_initial_input(initial_input)?; // before module globals execute
vm.set_storage(saved_namespaces)?;
for binding in collect_bindings(vm.scene()) { vm.attach(binding)?; }
let initialized = vm.initialize()?; // apply its scene AND commands
let output = vm.step(next_input)?;
```

`FrameInput` supplies clocks, screen/canvas size, cursor, 64 stereo audio bins, user values, settings and media events. `RuntimeOptions.user_property_definitions` supplies declared types, including Color→Vec3. Assets, the bounded VFS resource-name whitelist and immutable initial scene configurations live in shared Rust data; native Boa callbacks convert only requested resources. Modules are case folded and resolved from a supplied library, including the native `scripts/jsmodules/` prefix. Existing direct paths win; otherwise an exported `__workshopId` or the owning resource namespace resolves `category/workshop/id/…` paths. Descriptor-based `createLayer` creates an actual image/model/particle reference, while bulk creates and sorts coalesce the final object-array patch without changing order. No filesystem or network resolver exists inside the VM.

Implemented paths include typed vector/matrix/color math; property return/direct assignment; nested effect/material providers with canonical/alias shader uniforms; layer hierarchy and attachments; local/world bone transforms; animation/texture/video provider state; procedural typed vertex/index buffers and deferred handles; timers/cancellation; ordered audio, input and media callbacks; storage namespaces; script property builders and user links; initialization/update/destroy and dynamic script retirement/replacement. Dynamic module failures are isolated and suppressed until their source changes. Native device operations cross the explicit host command protocol in [the runtime](src/runtime.rs); the integrating renderer/model/particle/audio/platform service must actually perform them.

`advance_animation_clocks` also handles scenes with zero bindings. Serialized property curves change the actual wrapper value, including scalar/vector channels, saved cubic tangent units, stepped keys, relative values, wrap, single/loop/mirror, start-paused/random initialization and end callbacks. Random initialization uses the pinned MT19937 implementation and an explicit scene seed. Editor preview markers apply once and do not become authored values in the editor document.

Meaningful execution tests cover the major paths and failures. Dynamic material/bone/clip properties require real metadata; the service does not invent a successful device result when that metadata or a host provider is absent.

Loop, recursion, promise-job, module/source and output bounds are enforced. They do not implement a native fifteen-second watchdog, a hard JS heap budget, or arbitrary interruption of long intrinsic operations. Realms are per binding and share a retained Boa object: Vec/Mat prototypes, object aliases, cyclic objects and callable identities survive across bindings. Scene/storage output stays JSON; shared functions route host effects into the active frame transport while retaining their defining binding. Repeated ancestor scene patches coalesce to their final state; host commands keep call order. Saved spline geometry is reproduced, but full original native floating-point/overshooting-curve equivalence and original V8 behavior remain unproven.

## Validation and execution classification

- `tests/execution.rs` runs owned fixture modules in the new VM. Unit tests cover real property values, clock boundaries and the `LSKV0001` storage value envelope.
- `examples/corpus.rs` executes user-supplied inline bindings in the new bounded VM. It never starts an original engine or UI.
- Actual Mac execution, GPU results and complete original native numerical equivalence are not established by these tests.

From the workspace, run `cargo test -p wpe-script --locked` and `cargo check -p wpe-script --target aarch64-apple-darwin --locked` (or `x86_64-apple-darwin`). The validation ledger records which checks actually ran.
