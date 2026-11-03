# Native screen saver

`WallpaperMacScreenSaver` is a real `ScreenSaver.framework` subclass compiled as `wallpaper_screensaver` cdylib. The root packager supplies `WallpaperMac.saver` and its principal-class plist. `startAnimation`, `animateOneFrame` and `stopAnimation` create/tick/resize/tear down `WallpaperSurface`; the surface owns actual Metal, AV or WebKit providers. Preview and fullscreen use the same implementation. No placeholder animation is reported as wallpaper playback.

Build on a Mac with `./scripts/build-macos.sh native` (or `universal`), copy `dist/WallpaperMac.saver` to `~/Library/Screen Savers`, then select it in System Settings. Screen Saver Options provides project/shared-assets pickers, mute and system-audio-response settings. The app's Playback page opens the install directory. A project must be supplied by the user or the app's existing assignment.

Preferences are app-owned `~/Library/Application Support/WallpaperMac/screensaver.json`. Picked project/assets URLs preserve security-scoped bookmarks. Storage merges only changed keys under an advisory file lock before atomic save, preventing different display instances from overwriting unrelated keys. Display identities use public display UUIDs, with a separate preview key. Initial script context includes the actual view/backing scale and local time. Stop drops capture/media/GPU providers and releases bookmark scopes. Choosing another project clears its previous external-application runtime mapping.

Actual ScreenSaver.framework loading, Apple signing/notarization, host-process lifetime, preview/fullscreen rendering, TCC permissions, Korean/emoji output and concurrent multi-display behavior require the user's Mac. This Linux host has run **0** screen saver instances. Both Darwin target metadata checks do not link an SDK framework or load the plug-in.

External app wallpapers need an explicit runtime supplied in the saver configuration (or inherited from the first app assignment). Audio capture is opt-in and may be denied by the host process's privacy policy. Errors are displayed as errors, not successful playback.

Primary APIs: [ScreenSaverView](https://developer.apple.com/documentation/screensaver/screensaverview), [security-scoped bookmarks](https://developer.apple.com/documentation/foundation/nsurl/bookmarkcreationoptions/withsecurityscope). Reviewed 2026-10-28.
