# Wallpaper document editor service

The service edits a caller-owned loose working copy. It never starts the original editor/UI and does not change installed wallpaper files. The Mac host copies/unpacks a project into its editing area before creating `Editor`.

```rust,ignore
let mut editor = Editor::new(working_root, project_json, scene_json)?;
editor.execute("beginTransaction", json!([]))?;
editor.execute("addTextLayer", json!([{"text":"Hello"}]))?;
editor.execute("commitTransaction", json!([]))?;
editor.undo()?;
editor.redo()?;
editor.save()?;
```

Implemented local operations include scene/project/resource documents, layers and hierarchy order, materials/effects, particle element collections, model clip/event/attachment metadata, camera paths, expressions, bone masks, poses and texture variants. Image import writes an actual TEX→material→image-descriptor chain; PNG export decodes the actual texture. OBJ import triangulates faces, including negative indices, into the supported procedural model JSON. Sound/font/binary files are staged as real byte resources.

The bounded FBX decoder imports actual geometry, skin and sampled animation into the editable model profile. Normal model import stages the FBX source and its editable JSON; `import_fbx_for_resource` appends animation to an existing resource or scene model, matches unique bone names, and converts local keys by `targetRest × inverse(importedRest) × importedKey`. Existing rest/geometry and source MDL bytes remain intact. The source FBX and updated document form one undoable edit. Unsupported transforms, unmatched skeletons and excessive sampled output return explicit errors. Retarget output is capped at250,000 matrices across imported clips to bound memory.

Procedural geometry edits modify real vertex/index buffers: insertion splits incident triangles, edge opening splits connected islands, shortest-edge decimation removes degenerate triangles, interior removal retriangulates a simple outer boundary, weight smoothing/normalization computes new values, and automatic depth interpolates selected vertices from anchored neighbors. These are independent reconstruction algorithms, with explicit bounds and failures for holey or unsupported representations. They are not claims of original puppet-tool numeric equivalence or an original MDL writer.

`model_tools::decode_model_to_editable` converts actual MDL vertex/index/skin/bone/clip data into an editable sidecar. `upgradePuppetModel` stages that sidecar without rewriting the source MDL. Bone order updates actual editable triangle order while preserving parent/skin indices. Clip/event/attachment edits feed the real model animator and preview renderer; repeated unchanged preview metadata does not reset live animation. Owned binary fixtures prove that saved resources, rendered pixels and undo history change together.

Image operations produce real output pixels and staged resource bytes: gradient/Sobel normal maps, luminance depth, a color-distribution/min-cut foreground splitter, PNG separation masks and shelf-packed character atlases. Foreground/background alpha is complementary, painted seeds are preserved, and one operation is undoable. Depth and separation are explicitly independent procedural profiles; no unavailable original semantic neural model is represented as running. These bounded local operations are synchronous, so foreground compiler cancellation reports their actual idle state.

Painting is actual TEX pixel editing: `beginPaint(layerId, materialPath, slot, component, options)`, one or more `paintStroke({x,y,radius,opacity,hardness,color:[r,g,b,a]})`, then `endPaint()`. The whole gesture is one undo transaction. Native pointer-to-texture coordinate mapping belongs to the preview host. `clearPaint` edits actual channels; canceling a gesture restores staged bytes.

Undo/redo stores inverse JSON edits and byte-resource changes. Transactions group commands, failed edits restore their prior state, rollback restores dirty/timer state, and undo to saved content clears dirty. Autosave waits ten seconds after a change and pauses during a transaction. Saves prepare temporary files, replace individual files, clean temporary outputs on failure, and attempt rollback after a commit error. A power failure during a multi-file replacement is not claimed to be globally atomic. Paths are relative and bounded, including symlink checks; mounts are read-only until an edit creates a local shadow copy.

`preview_scene()` returns a scene copy with transient animation seek/play controls. Feed this to the actual ScenePlayer preview. `snapshot().scene` remains the authored save document. Temporary project input produces a `previewProjectData` event.

## Host services and coverage

The [command dispatcher](src/commands.rs) implements local/session/host/error routes. A registered route does not establish original behavior equality for every method.

External commands use a real request/completion protocol. A reply contains `{pending:true,request_id}` and an event `{kind:"hostCommand",payload:{request_id,method,args}}`. The provider performs the operation, then calls `complete_host_request(id, Ok(value))` or `Err(message)`. Completion emits `hostCompleted`. The broker does not count Steam, Workshop publishing, DLC installation, native pickers, shader preview/export, AI image tools, or external applications as completed capabilities by itself. Model-resource actions that cannot be represented as editable JSON require such a provider.

The current routing ledger distinguishes 39 host methods, 18 session methods, 15 local/host hybrids and 93 local document/resource operations. The action ledger distinguishes 103 local data/geometry algorithms, 13 session actions and 15 host requests. Native host services must complete the latter with an actual result or error. Route counts do not establish an original behavior-equivalence percentage. Actual Mac providers and UI integration must be tested separately.

`cargo test -p wpe-editor --locked` validates real saved files, decoded image pixels, undo/redo/transactions, topology/weights/depth, path rejection, preview separation, resource handles and pending-provider failure handling. No original native editor, original UI or real Mac GPU is executed in those tests.
