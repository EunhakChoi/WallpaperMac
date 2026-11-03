#![cfg(target_os = "linux")]
use std::{
    ffi::{CString, c_void},
    path::PathBuf,
};
use wpe_platform::steam::{ItemUpdate, SteamConfig, SteamProvider};
#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(path: *const i8, flags: i32) -> *mut c_void;
    fn dlsym(module: *mut c_void, name: *const i8) -> *mut c_void;
    fn dlclose(module: *mut c_void) -> i32;
}
#[test]
fn configured_dylib_calls_release_callbacks_queries_shutdown_and_mutations() {
    let dir = std::env::temp_dir().join(format!("wpe-steam-ffi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let library = dir.join("owned-mock.so");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/steam_mock.c");
    let compiled = std::process::Command::new("cc")
        .args([
            "-shared", "-fPIC", "-std=c11", "-Werror", "-Wall", "-Wextra",
        ])
        .arg(source)
        .arg("-o")
        .arg(&library)
        .status()
        .unwrap();
    assert!(compiled.success());
    let name = CString::new(library.to_str().unwrap()).unwrap();
    let monitor = unsafe { dlopen(name.as_ptr(), 2) };
    assert!(!monitor.is_null());
    let reset: unsafe extern "C" fn(i32) =
        unsafe { std::mem::transmute(dlsym(monitor, c"MockReset".as_ptr())) };
    let counter: unsafe extern "C" fn(i32) -> i32 =
        unsafe { std::mem::transmute(dlsym(monitor, c"MockCounter".as_ptr())) };
    let config = SteamConfig {
        library: library.clone(),
        app_id: 431960,
    };
    unsafe {
        reset(0);
    }
    let mut steam = SteamProvider::load(&config).unwrap();
    assert!(SteamProvider::load(&config).is_err());
    assert_eq!(steam.subscribed().unwrap(), [99]);
    assert_eq!(steam.dlcs().unwrap()[0].id, 123);
    assert!(!steam.dlc(123).unwrap().installed);
    assert!(steam.install_dlc(123, true).unwrap().installed);
    assert!(!steam.install_dlc(123, false).unwrap().installed);
    assert_eq!(steam.dlc(123).unwrap().progress, Some((30, 100)));
    assert!(steam.install_dlc(999, true).is_err());
    assert_eq!(
        steam.unlock_publish_achievement("scene").unwrap()["stored"],
        false
    );
    let achievements = steam.pump();
    assert!(
        achievements
            .iter()
            .any(|e| e.kind == "achievement" && e.ok && e.value["stored"] == true)
    );
    assert_eq!(unsafe { counter(4) }, 1);
    assert_eq!(unsafe { counter(5) }, 1);
    assert_eq!(steam.install_info(99).unwrap().unwrap().bytes, 123);
    let call = steam.query_details(&[99]).unwrap();
    let result = steam.pump();
    assert_eq!(result[0].operation, call);
    assert!(result[0].ok);
    assert_eq!(result[0].value["items"][0]["title"], "Mock title");
    assert_eq!(unsafe { counter(1) }, 1);
    assert_eq!(unsafe { counter(2) }, 3);
    steam.query_published(1).unwrap();
    assert!(steam.pump()[0].ok);
    steam
        .query_filtered(
            1,
            0,
            -1,
            Some("fixture"),
            &["Scene".into()],
            &["Mature".into()],
        )
        .unwrap();
    assert!(steam.pump()[0].ok);
    steam.delete_item(99).unwrap();
    let deleted = steam.pump();
    assert!(deleted[0].ok);
    assert_eq!(deleted[0].kind, "delete");
    assert_eq!(deleted[0].value["id"], 99);
    steam.download(99, true).unwrap();
    assert!(steam.pump()[0].ok);
    assert_eq!(steam.download_progress(99), Some((20, 100)));
    for enabled in [true, false] {
        steam.subscribe(99, enabled).unwrap();
        assert!(steam.pump()[0].ok);
    }
    steam.create().unwrap();
    let created = steam.pump();
    assert_eq!(created[0].value["id"], 123456);
    assert_eq!(created[0].value["needs_workshop_agreement"], true);
    steam
        .update(&ItemUpdate {
            id: 99,
            content: dir.clone(),
            title: Some("Title".into()),
            description: Some("Description".into()),
            tags: vec!["Scene".into()],
            visibility: Some(0),
            change_note: "Update".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(steam.pending_progress()[0]["phase"], 4);
    assert!(steam.pump()[0].ok);
    assert_eq!(unsafe { counter(3) }, 1);
    drop(steam);
    assert_eq!(unsafe { counter(0) }, 1);
    unsafe {
        reset(9);
    }
    let optional = SteamProvider::load(&config).unwrap();
    assert!(optional.dlcs().is_err());
    drop(optional);
    for mode in [3, 4, 5, 8] {
        unsafe {
            reset(mode);
        }
        let mut steam = SteamProvider::load(&config).unwrap();
        steam.query_details(&[99]).unwrap();
        let result = steam.pump();
        assert_eq!(result.len(), 1);
        assert!(!result[0].ok, "mode{mode}");
        assert_eq!(unsafe { counter(2) }, 1);
        drop(steam);
        assert_eq!(unsafe { counter(1) }, 1);
        assert_eq!(unsafe { counter(0) }, 1);
    }
    unsafe {
        reset(1);
    }
    let mut steam = SteamProvider::load(&config).unwrap();
    steam.query_page(1, 0, 0).unwrap();
    drop(steam);
    assert_eq!(unsafe { counter(1) }, 1);
    unsafe {
        reset(0);
    }
    assert!(
        SteamProvider::load(&SteamConfig {
            library,
            app_id: 123
        })
        .is_err()
    );
    assert_eq!(unsafe { counter(0) }, 1);
    for mode in [11, 12, 13, 14, 15] {
        unsafe {
            reset(mode);
        }
        let mut steam = SteamProvider::load(&config).unwrap();
        let result = steam.unlock_publish_achievement("scene");
        if mode == 15 {
            assert!(result.is_err());
        } else {
            assert!(result.is_ok());
            let events = steam.pump();
            if mode == 11 || mode == 12 {
                assert!(events.iter().any(|e| e.kind == "achievement" && !e.ok));
                assert!(!events.iter().any(|e| e.kind == "achievement" && e.ok));
            } else {
                assert!(events.is_empty());
                assert_eq!(unsafe { counter(4) }, 0);
            }
        }
        drop(steam);
    }
    unsafe {
        dlclose(monitor);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
