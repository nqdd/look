use crate::config::RuntimeConfig;
use crate::index::APP_CANDIDATE_ID_PREFIX;
use crate::platform::macos;
use crate::platform::paths::{candidate_id_path_component, path_is_same_or_child};
use look_indexing::{Candidate, CandidateKind};
use plist::Value;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::mpsc;

pub(crate) fn discover_installed_apps(config: &RuntimeConfig, tx: mpsc::SyncSender<Candidate>) {
    for root in merged_app_scan_roots(
        &config.app_scan_roots,
        &macos::additional_app_scan_roots(),
        macos::REQUIRED_APP_SCAN_ROOTS,
    ) {
        walk_apps(
            &root,
            config.app_scan_depth,
            &tx,
            &config.app_exclude_paths,
            &config.app_exclude_names,
        );
    }
}

fn merged_app_scan_roots(
    config_roots: &[String],
    additional_roots: &[String],
    required_roots: &[&str],
) -> Vec<String> {
    let mut out =
        Vec::with_capacity(config_roots.len() + additional_roots.len() + required_roots.len());
    let mut seen =
        HashSet::with_capacity(config_roots.len() + additional_roots.len() + required_roots.len());
    for root in config_roots.iter().chain(additional_roots.iter()) {
        let normalized = candidate_id_path_component(root);
        if seen.insert(normalized) {
            out.push(root.clone());
        }
    }

    for root in required_roots {
        let normalized = candidate_id_path_component(root);
        if seen.insert(normalized) {
            out.push((*root).to_string());
        }
    }

    out
}

fn walk_apps(
    path: &str,
    depth: usize,
    tx: &mpsc::SyncSender<Candidate>,
    app_exclude_paths: &[String],
    app_exclude_names: &[String],
) {
    if should_exclude_path(path, app_exclude_paths) {
        return;
    }

    if depth == 0 {
        return;
    }

    let Ok(entries) = fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        let app_path = entry.path();
        let Some(app_path_str) = app_path.to_str() else {
            continue;
        };
        if should_exclude_path(app_path_str, app_exclude_paths) {
            continue;
        }

        if app_path_str.ends_with(".app") {
            send_app_candidate(&app_path, &file_type, app_path_str, tx, app_exclude_names);
        } else if file_type.is_dir() {
            walk_apps(
                app_path_str,
                depth - 1,
                tx,
                app_exclude_paths,
                app_exclude_names,
            );
        }
    }
}

fn send_app_candidate(
    app_path: &Path,
    file_type: &fs::FileType,
    app_path_str: &str,
    tx: &mpsc::SyncSender<Candidate>,
    app_exclude_names: &[String],
) {
    if !is_app_bundle(file_type, app_path) {
        return;
    }

    let fallback_title = app_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("App");
    let title = app_title(app_path, fallback_title);
    if should_exclude_app_name(&title, app_exclude_names)
        || should_exclude_app_name(fallback_title, app_exclude_names)
    {
        return;
    }

    let key = format!(
        "{APP_CANDIDATE_ID_PREFIX}{}",
        candidate_id_path_component(app_path_str)
    );
    let _ = tx.send(Candidate::new(
        &key,
        CandidateKind::App,
        &title,
        app_path_str,
    ));
}

fn is_app_bundle(file_type: &fs::FileType, app_path: &Path) -> bool {
    file_type.is_dir()
        || (file_type.is_symlink()
            && fs::metadata(app_path)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false))
}

fn app_title(app_path: &Path, fallback_title: &str) -> String {
    title_from_info_plist(app_path).unwrap_or_else(|| fallback_title.to_string())
}

fn title_from_info_plist(app_path: &Path) -> Option<String> {
    let info_plist = app_path.join("Contents").join("Info.plist");
    let value = Value::from_file(info_plist).ok()?;
    let dict = value.as_dictionary()?;
    ["CFBundleDisplayName", "CFBundleName"]
        .into_iter()
        .find_map(|key| plist_string(dict.get(key)))
}

fn plist_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_string)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn should_exclude_path(path: &str, app_exclude_paths: &[String]) -> bool {
    app_exclude_paths.iter().any(|entry| {
        let normalized_exclude = entry.trim();
        if normalized_exclude.is_empty() {
            return false;
        }
        path_is_same_or_child(path, normalized_exclude)
    })
}

fn should_exclude_app_name(name: &str, app_exclude_names: &[String]) -> bool {
    let normalized_name = name.trim().trim_end_matches(".app").trim().to_lowercase();
    app_exclude_names.iter().any(|entry| {
        let normalized_exclude = entry.trim().trim_end_matches(".app").trim().to_lowercase();
        !normalized_exclude.is_empty() && normalized_exclude == normalized_name
    })
}

#[cfg(test)]
mod tests {
    use super::{
        app_title, merged_app_scan_roots, should_exclude_app_name, should_exclude_path, walk_apps,
    };
    use look_indexing::Candidate;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "look-macos-apps-{name}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn create_app(root: &Path, name: &str, display_name: Option<&str>) -> PathBuf {
        let app = root.join(name);
        let contents = app.join("Contents");
        fs::create_dir_all(&contents).expect("create app contents");
        if let Some(display_name) = display_name {
            fs::write(
                contents.join("Info.plist"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDisplayName</key>
    <string>{display_name}</string>
    <key>CFBundleName</key>
    <string>Fallback Name</string>
</dict>
</plist>
"#
                ),
            )
            .expect("write Info.plist");
        }
        app
    }

    #[cfg(unix)]
    fn symlink_dir(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("create symlink");
    }

    fn collect_apps(root: &Path) -> Vec<Candidate> {
        let (tx, rx) = mpsc::sync_channel(16);
        let empty = Vec::<String>::new();
        walk_apps(
            root.to_str().expect("utf-8 temp path"),
            3,
            &tx,
            &empty,
            &empty,
        );
        drop(tx);
        rx.into_iter().collect()
    }

    #[test]
    fn excludes_app_paths_by_prefix() {
        let excludes = vec!["/Applications/Utilities".to_string()];
        assert!(should_exclude_path("/Applications/Utilities", &excludes));
        assert!(should_exclude_path(
            "/Applications/Utilities/Terminal.app",
            &excludes
        ));
    }

    #[test]
    fn excludes_app_names_case_insensitively() {
        let names = vec!["safari".to_string(), "Visual Studio Code".to_string()];
        assert!(should_exclude_app_name("Safari", &names));
        assert!(should_exclude_app_name("Visual Studio Code.app", &names));
        assert!(!should_exclude_app_name("Calculator", &names));
    }

    #[test]
    fn ignores_blank_exclude_entries() {
        let excludes = vec!["  ".to_string(), "".to_string()];
        assert!(!should_exclude_path("/Applications/Utilities", &excludes));

        let names = vec![" ".to_string(), "".to_string()];
        assert!(!should_exclude_app_name("Safari", &names));
    }

    #[test]
    fn path_prefix_is_boundary_aware() {
        let excludes = vec!["/Applications/Util".to_string()];
        assert!(!should_exclude_path("/Applications/Utilities", &excludes));
    }

    #[test]
    fn merged_roots_preserve_order_and_deduplicate() {
        let roots = vec!["/Applications".to_string()];
        let additional = vec![
            "/Users/demo/Applications".to_string(),
            "/Applications/".to_string(),
        ];

        let required = vec!["/System/Library/CoreServices/Applications", "/Applications"];

        let merged = merged_app_scan_roots(&roots, &additional, &required);
        assert_eq!(
            merged,
            vec![
                "/Applications".to_string(),
                "/Users/demo/Applications".to_string(),
                "/System/Library/CoreServices/Applications".to_string()
            ]
        );
    }

    #[test]
    fn app_title_prefers_bundle_display_name() {
        let tmp = TempDir::new("title");
        let app = create_app(tmp.path(), "Client Riot.app", Some("Riot Client"));

        assert_eq!(app_title(&app, "Client Riot"), "Riot Client");
    }

    #[test]
    fn app_title_falls_back_to_file_stem() {
        let tmp = TempDir::new("fallback-title");
        let app = create_app(tmp.path(), "Client Riot.app", None);

        assert_eq!(app_title(&app, "Client Riot"), "Client Riot");
    }

    #[test]
    #[cfg(unix)]
    fn indexes_symlinked_app_bundle() {
        let tmp = TempDir::new("symlink-app");
        let real_root = tmp.path().join("Real Apps");
        let scan_root = tmp.path().join("Applications");
        fs::create_dir_all(&real_root).expect("create real root");
        fs::create_dir_all(&scan_root).expect("create scan root");
        let real_app = create_app(&real_root, "Riot Client.app", Some("Riot Client"));
        let link_app = scan_root.join("Client Riot.app");
        symlink_dir(&real_app, &link_app);

        let apps = collect_apps(&scan_root);

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].title.as_ref(), "Riot Client");
        assert_eq!(
            apps[0].path.as_ref(),
            link_app.to_str().expect("utf-8 symlink path")
        );
    }

    #[test]
    #[cfg(unix)]
    fn does_not_recurse_into_non_app_symlinked_directory() {
        let tmp = TempDir::new("symlink-dir");
        let real_root = tmp.path().join("Real Apps");
        let scan_root = tmp.path().join("Applications");
        fs::create_dir_all(&real_root).expect("create real root");
        fs::create_dir_all(&scan_root).expect("create scan root");
        create_app(&real_root, "Nested.app", Some("Nested"));
        symlink_dir(&real_root, &scan_root.join("Linked Apps"));

        let apps = collect_apps(&scan_root);

        assert!(apps.is_empty());
    }

    #[test]
    fn excludes_app_by_bundle_filename_even_when_display_name_differs() {
        let tmp = TempDir::new("exclude-filename");
        create_app(tmp.path(), "Client Riot.app", Some("Riot Client"));
        let (tx, rx) = mpsc::sync_channel(16);
        let empty = Vec::<String>::new();
        walk_apps(
            tmp.path().to_str().expect("utf-8 temp path"),
            3,
            &tx,
            &empty,
            &["Client Riot".to_string()],
        );
        drop(tx);

        assert!(rx.into_iter().collect::<Vec<_>>().is_empty());
    }
}
