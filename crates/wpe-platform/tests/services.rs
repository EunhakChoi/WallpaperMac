use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};
use wpe_platform::{config::*, playback::*, service::*, workshop};
fn temporary(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("wpe-platform-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn selection(name: &str) -> Selection {
    Selection {
        project: name.into(),
        ..Selection::default()
    }
}
#[test]
fn save_reload_and_rejected_config_leave_disk_valid() {
    let dir = temporary("config");
    let store = ConfigStore::new(dir.join("config.json"));
    let mut cfg = AppConfig {
        fps: 60,
        ..AppConfig::default()
    };
    cfg.screens.insert("display-1".into(), selection("demo"));
    store.save(&cfg).unwrap();
    cfg.fps = 0;
    assert!(store.save(&cfg).is_err());
    assert_eq!(store.load().unwrap().fps, 60);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn rule_exemption_preserves_other_audio_and_last_matching_load() {
    let policy = PlaybackPolicy {
        audio: Action::PauseAll,
        ..PlaybackPolicy::default()
    };
    let rules = vec![
        AppRule {
            executable: "music".into(),
            condition: Condition::PlayingAudio,
            action: RuleAction::Run,
        },
        AppRule {
            executable: "game".into(),
            condition: Condition::Running,
            action: RuleAction::LoadWallpaper("first".into()),
        },
        AppRule {
            executable: "game.exe".into(),
            condition: Condition::Running,
            action: RuleAction::LoadWallpaper("last".into()),
        },
    ];
    let mut facts = PlaybackFacts {
        active_displays: 3,
        audio_active: true,
        applications: vec![
            ApplicationFact {
                executable: "music.exe".into(),
                playing_audio: true,
                ..Default::default()
            },
            ApplicationFact {
                executable: "game".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let first = decide(&policy, &rules, &facts);
    assert_eq!(first.paused, 0);
    assert_eq!(first.load_rule, Some(2));
    facts.applications.push(ApplicationFact {
        executable: "other".into(),
        playing_audio: true,
        ..Default::default()
    });
    assert_eq!(decide(&policy, &rules, &facts).paused, 3);
}
#[test]
fn failed_profile_is_transactional_and_rule_exit_restores_slots() {
    let mut service = PlaybackService::default();
    let rules = vec![AppRule {
        executable: "x".into(),
        condition: Condition::Running,
        action: RuleAction::LoadProfile("test".into()),
    }];
    let mut slots = BTreeMap::from([("one".into(), selection("old"))]);
    let d = Decision {
        load_rule: Some(0),
        ..Default::default()
    };
    assert!(service.apply_rules(&d, &rules, &[], &mut slots).is_err());
    assert_eq!(slots["one"].project, PathBuf::from("old"));
    let profiles = vec![Profile {
        name: "test".into(),
        screens: BTreeMap::from([("two".into(), selection("new"))]),
    }];
    assert!(
        service
            .apply_rules(&d, &rules, &profiles, &mut slots)
            .unwrap()
    );
    assert!(slots.contains_key("two"));
    assert!(
        service
            .apply_rules(&Decision::default(), &rules, &profiles, &mut slots)
            .unwrap()
    );
    assert!(slots.contains_key("one"));
}
#[test]
fn playlist_timer_pause_and_calendar_are_deterministic() {
    let p = Playlist {
        name: "p".into(),
        settings: PlaylistSettings {
            interval_seconds: 10.0,
            ..Default::default()
        },
        items: vec![
            PlaylistItem {
                selection: selection("one"),
                daytime_end: Some(720),
                weekdays: vec![1],
            },
            PlaylistItem {
                selection: selection("two"),
                daytime_end: Some(1439),
                weekdays: vec![2],
            },
        ],
    };
    let c = Calendar {
        minute: 780,
        weekday: 2,
        day: 1,
    };
    let mut s = PlaylistState::default();
    assert_eq!(
        s.advance(&p, 0.0, c, false).unwrap().project,
        PathBuf::from("one")
    );
    assert!(s.advance(&p, 9.0, c, false).is_none());
    assert_eq!(
        s.advance(&p, 1.0, c, false).unwrap().project,
        PathBuf::from("two")
    );
    let mut day = p.clone();
    day.settings.mode = PlaylistMode::Daytime;
    assert_eq!(
        PlaylistState::default()
            .advance(&day, 0.0, c, false)
            .unwrap()
            .project,
        PathBuf::from("two")
    );
}
#[test]
fn random_playlist_visits_each_item_before_repeating() {
    let p = Playlist {
        name: "p".into(),
        settings: PlaylistSettings {
            order: Order::Random,
            ..Default::default()
        },
        items: (0..5)
            .map(|i| PlaylistItem {
                selection: selection(&i.to_string()),
                daytime_end: None,
                weekdays: vec![],
            })
            .collect(),
    };
    let mut s = PlaylistState::default();
    let c = Calendar {
        minute: 0,
        weekday: 0,
        day: 0,
    };
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..5 {
        seen.insert(s.advance(&p, 0.0, c, true).unwrap().project);
    }
    assert_eq!(seen.len(), 5);
}
#[test]
fn management_assign_properties_profile_and_inventory() {
    let dir = temporary("service");
    let project = dir.join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        project.join("project.json"),
        r#"{"title":"Demo","type":"web","file":"index.html"}"#,
    )
    .unwrap();
    std::fs::write(project.join("index.html"), "demo").unwrap();
    let mut service = PlatformService::new(dir.join("config.json")).unwrap();
    service
        .execute("library.add", json!({"path":project}))
        .unwrap();
    assert_eq!(service.inventory.items.len(), 1);
    let reply = service
        .execute(
            "assign",
            json!({"screen":"1","selection":{"project":project}}),
        )
        .unwrap();
    assert!(matches!(&reply.actions[0], HostAction::Assign { .. }));
    service
        .execute(
            "properties.set",
            json!({"screen":"1","properties":{"speed":2}}),
        )
        .unwrap();
    service
        .execute("profile.save", json!({"name":"Mine"}))
        .unwrap();
    assert_eq!(
        service.config.profiles[0].screens["1"].properties["speed"],
        2
    );
    assert!(service.execute("config.set", json!({"fps":999})).is_err());
    assert_eq!(service.config.fps, 30);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn steam_request_and_response_do_not_accept_another_application() {
    let request = workshop::details_request(&[123, 456]).unwrap();
    assert_eq!(request.method, "POST");
    assert!(String::from_utf8_lossy(&request.body).contains("publishedfileids%5B0%5D=123"));
    let value = json!({"response":{"publishedfiledetails":[{"result":1,"publishedfileid":"123","consumer_app_id":431960,"title":"yes"},{"result":1,"publishedfileid":"456","consumer_app_id":1,"title":"no"}]}});
    let result = workshop::parse_details(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, 123);
}
#[test]
fn session_and_global_audio_holds_use_separate_expiry_windows() {
    let mut audio = AudioActivity::default();
    audio.observe_process(1.0, "Player", 0.002);
    audio.observe_global(1.0, 0.002);
    assert!(audio.process_active(3.999, "player.exe"));
    assert!(!audio.process_active(4.0, "player"));
    assert!(audio.global_active(5.999));
    assert!(!audio.global_active(6.0));
    audio.observe_global(7.0, f32::from_bits(0x3a83126f));
    assert!(!audio.global_active(7.0));
}
#[test]
fn editor_import_edits_an_owned_copy_and_keeps_source_bytes() {
    let source = temporary("original");
    let owned = temporary("editor-workspace");
    std::fs::write(source.join("scene.json"), b"original").unwrap();
    let copy = wpe_platform::library::working_copy(&source, &owned).unwrap();
    std::fs::write(copy.join("scene.json"), b"changed").unwrap();
    assert_eq!(
        std::fs::read(source.join("scene.json")).unwrap(),
        b"original"
    );
    assert_eq!(
        wpe_platform::library::working_copy(&source, &owned).unwrap(),
        copy
    );
    assert!(wpe_platform::library::working_copy(&source, &source.join("nested")).is_err());
    std::fs::remove_dir_all(source).unwrap();
    std::fs::remove_dir_all(owned).unwrap();
}
#[test]
fn pcm_integer_float_and_planar_conversion_has_defined_endpoints() {
    use wpe_platform::audio::PcmEncoding as P;
    assert_eq!(
        P::Signed {
            bits: 16,
            storage: 2,
            high_aligned: false
        }
        .decode(&[0, 128], false)
        .unwrap(),
        -1.
    );
    assert_eq!(
        P::Signed {
            bits: 24,
            storage: 3,
            high_aligned: false
        }
        .decode(&[128, 0, 0], true)
        .unwrap(),
        -1.
    );
    assert_eq!(
        P::Signed {
            bits: 24,
            storage: 4,
            high_aligned: true
        }
        .decode(&[0, 0, 0, 128], false)
        .unwrap(),
        -1.
    );
    assert_eq!(
        P::Float64.decode(&0.25f64.to_le_bytes(), false).unwrap(),
        0.25
    );
    assert_eq!(
        P::Float32.decode(&f32::NAN.to_le_bytes(), false).unwrap(),
        0.
    );
    assert_eq!(P::Unsigned8.decode(&[128], false).unwrap(), 0.);
}
#[test]
fn library_legacy_kind_inference_and_saved_copy_preserve_explicit_unknown() {
    let dir = temporary("legacy-library");
    for (name, metadata) in [
        ("legacy", json!({"file":"scene.json","title":"Legacy"})),
        ("mixed", json!({"type":"Scene","file":"scene.json"})),
        (
            "unknown",
            json!({"type":"new-foreign-kind","file":"scene.json"}),
        ),
    ] {
        let root = dir.join(name);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("project.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("scene.json"), b"{\"objects\":[]}").unwrap();
    }
    let scan = wpe_platform::library::scan(std::slice::from_ref(&dir));
    assert_eq!(
        scan.items
            .iter()
            .find(|i| i.title == "Legacy")
            .unwrap()
            .kind,
        "scene"
    );
    assert_eq!(
        scan.items
            .iter()
            .find(|i| i.root.ends_with("mixed"))
            .unwrap()
            .kind,
        "scene"
    );
    assert_eq!(
        scan.items
            .iter()
            .find(|i| i.root.ends_with("unknown"))
            .unwrap()
            .kind,
        "new-foreign-kind"
    );
    let output = dir.with_file_name(format!(
        "{}-export",
        dir.file_name().unwrap().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&output);
    wpe_platform::library::export_copy(&dir.join("legacy"), &output).unwrap();
    assert_eq!(
        std::fs::read(output.join("scene.json")).unwrap(),
        b"{\"objects\":[]}"
    );
    assert!(wpe_platform::library::export_copy(&dir.join("legacy"), &output).is_err());
    std::fs::remove_dir_all(output).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[cfg(unix)]
#[test]
fn external_encoder_finishes_exact_rgba_and_cancels_owned_process() {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    use wpe_platform::export::{ExportSettings, FfmpegEncoder};
    let dir = temporary("encoder plumbing");
    let script = dir.join("owned encoder.py");
    std::fs::write(&script,b"#!/usr/bin/python3\nimport sys,time\noutput=sys.argv[-1]\nif output.endswith('cancel.avi'):\n while True: time.sleep(1)\ndata=sys.stdin.buffer.read()\nwith open(output,'wb') as file: file.write(data)\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let settings = ExportSettings {
        width: 2,
        height: 2,
        fps: 2,
        duration: 1.,
        format: "avi".into(),
        ..Default::default()
    };
    let output = dir.join("frames.avi");
    let mut encoder = FfmpegEncoder::new(&script, &output, &settings).unwrap();
    for value in [37, 92] {
        let mut frame = vec![value; 16];
        let start = Instant::now();
        while let Some(returned) = encoder.submit(frame).unwrap() {
            frame = returned;
            assert!(start.elapsed().as_secs() < 3);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    encoder.finish();
    let start = Instant::now();
    while !encoder.status().finished {
        assert!(start.elapsed().as_secs() < 3);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(encoder.status().error.is_none());
    assert_eq!(encoder.status().written, 2);
    assert_eq!(
        std::fs::read(&output).unwrap(),
        [vec![37; 16], vec![92; 16]].concat()
    );
    assert!(FfmpegEncoder::new(&script, &output, &settings).is_err());
    let mut cancel = FfmpegEncoder::new(&script, &dir.join("cancel.avi"), &settings).unwrap();
    assert!(cancel.submit(vec![1; 16]).unwrap().is_none());
    cancel.cancel();
    let start = Instant::now();
    while !cancel.status().finished {
        assert!(start.elapsed().as_secs() < 3);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(cancel.status().error.is_some());
    drop(cancel);
    drop(encoder);
    std::fs::remove_dir_all(dir).unwrap();
}
