//! Finding the NSS databases browsers keep their certificates in.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Browser {
    Firefox,
    Zen,
    /// Chromium, Chrome and Brave share `~/.pki/nssdb`.
    Chromium,
    /// A directory named with `--profile`.
    Custom,
}

impl Browser {
    pub fn label(self) -> &'static str {
        match self {
            Self::Firefox => "firefox",
            Self::Zen => "zen",
            Self::Chromium => "chromium",
            Self::Custom => "profile",
        }
    }

    /// Browsers that hold a profile lock while running and read `user.js`.
    pub fn is_gecko(self) -> bool {
        matches!(self, Self::Firefox | Self::Zen | Self::Custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub browser: Browser,
    pub name: String,
    pub dir: PathBuf,
}

/// The profiles a `profiles.ini` lists, as `(name, directory)`. A profile is
/// relative to the file's directory only when `IsRelative=1`, as in Firefox.
pub fn parse_profiles_ini(text: &str, root: &Path) -> Vec<(String, PathBuf)> {
    #[derive(Default)]
    struct Section {
        is_profile: bool,
        name: Option<String>,
        path: Option<String>,
        relative: bool,
    }
    fn finish(section: Section, root: &Path, out: &mut Vec<(String, PathBuf)>) {
        if !section.is_profile {
            return;
        }
        let Some(path) = section.path.filter(|p| !p.is_empty()) else {
            return;
        };
        let dir = if section.relative {
            root.join(&path)
        } else {
            PathBuf::from(&path)
        };
        out.push((section.name.unwrap_or(path), dir));
    }

    let mut out = Vec::new();
    let mut current = Section::default();
    for line in text.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            finish(std::mem::take(&mut current), root, &mut out);
            current.is_profile = header.starts_with("Profile");
        } else if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "Name" => current.name = Some(value.trim().to_owned()),
                "Path" => current.path = Some(value.trim().to_owned()),
                "IsRelative" => current.relative = value.trim() == "1",
                _ => {}
            }
        }
    }
    finish(current, root, &mut out);
    out
}

/// Where each browser keeps things, all derived from `$HOME` and
/// `$XDG_CONFIG_HOME`.
#[derive(Debug, Clone)]
pub struct Roots {
    pub home: PathBuf,
    pub xdg_config: PathBuf,
}

impl Roots {
    pub fn new(home: &Path, xdg_config_home: Option<&Path>) -> Self {
        let xdg_config = match xdg_config_home {
            Some(dir) if dir.is_absolute() => dir.to_owned(),
            _ => home.join(".config"),
        };
        Self {
            home: home.to_owned(),
            xdg_config,
        }
    }

    fn gecko_roots(&self, browser: Browser) -> Vec<PathBuf> {
        let app_support = self.home.join("Library").join("Application Support");
        match browser {
            Browser::Firefox => vec![
                self.home.join(".mozilla").join("firefox"),
                self.xdg_config.join("mozilla").join("firefox"),
                app_support.join("Firefox"),
            ],
            Browser::Zen => vec![
                self.home.join(".zen"),
                self.xdg_config.join("zen"),
                app_support.join("zen"),
            ],
            Browser::Chromium | Browser::Custom => Vec::new(),
        }
    }
}

/// Every NSS directory for the chosen browsers plus the explicit ones, each
/// once. A browser that is not installed contributes nothing.
pub fn discover(roots: &Roots, browsers: &[Browser], explicit: &[PathBuf]) -> Vec<Target> {
    let mut found: Vec<Target> = Vec::new();
    let mut push = |target: Target| {
        let key = target
            .dir
            .canonicalize()
            .unwrap_or_else(|_| target.dir.clone());
        let duplicate = found
            .iter()
            .any(|t| t.dir.canonicalize().unwrap_or_else(|_| t.dir.clone()) == key);
        if !duplicate {
            found.push(target);
        }
    };
    for &browser in browsers {
        for root in roots.gecko_roots(browser) {
            let Ok(ini) = std::fs::read_to_string(root.join("profiles.ini")) else {
                continue;
            };
            for (name, dir) in parse_profiles_ini(&ini, &root) {
                if dir.is_dir() {
                    push(Target { browser, name, dir });
                }
            }
        }
        if browser == Browser::Chromium {
            let dir = roots.home.join(".pki").join("nssdb");
            if dir.is_dir() {
                push(Target {
                    browser,
                    name: "nssdb".to_owned(),
                    dir,
                });
            }
        }
    }
    for dir in explicit {
        push(Target {
            browser: Browser::Custom,
            name: dir.file_name().map_or_else(
                || "profile".to_owned(),
                |n| n.to_string_lossy().into_owned(),
            ),
            dir: dir.clone(),
        });
    }
    found
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn relative_and_absolute_profiles_resolve_against_the_ini_directory() {
        let ini = "[General]\nVersion=2\n\n[Profile0]\nName=work\nIsRelative=1\nPath=abc.work\n\n\
                   [Profile1]\nName=elsewhere\nIsRelative=0\nPath=/data/ff\n\n\
                   [Install123]\nDefault=abc.work\n";
        let got = parse_profiles_ini(ini, Path::new("/home/u/.mozilla/firefox"));
        assert_eq!(
            got,
            vec![
                (
                    "work".to_owned(),
                    PathBuf::from("/home/u/.mozilla/firefox/abc.work")
                ),
                ("elsewhere".to_owned(), PathBuf::from("/data/ff")),
            ]
        );
    }

    #[test]
    fn a_profile_without_a_path_or_is_relative_is_handled_like_firefox() {
        let ini =
            "[Profile0]\nName=nopath\n[Profile1]\nPath=rel\n[Profile2]\nPath=zero\nIsRelative=0\n";
        let got = parse_profiles_ini(ini, Path::new("/r"));
        assert_eq!(
            got,
            vec![
                ("rel".to_owned(), PathBuf::from("rel")),
                ("zero".to_owned(), PathBuf::from("zero")),
            ]
        );
    }

    fn make_profile(root: &Path, ini_body: &str, dirs: &[&str]) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("profiles.ini"), ini_body).unwrap();
        for d in dirs {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
    }

    #[test]
    fn firefox_zen_and_chromium_are_found_from_home_and_xdg() {
        let home = TempDir::new().unwrap();
        let h = home.path();
        make_profile(
            &h.join(".mozilla/firefox"),
            "[Profile0]\nName=a\nIsRelative=1\nPath=a.default\n[Profile1]\nName=b\nIsRelative=1\nPath=b.default\n",
            &["a.default", "b.default"],
        );
        make_profile(
            &h.join(".config/mozilla/firefox"),
            "[Profile0]\nName=x\nIsRelative=1\nPath=x\n",
            &["x"],
        );
        make_profile(
            &h.join(".config/zen"),
            "[Profile0]\nName=z\nIsRelative=1\nPath=z\n",
            &["z"],
        );
        std::fs::create_dir_all(h.join(".pki/nssdb")).unwrap();

        let roots = Roots::new(h, None);
        let all = [Browser::Firefox, Browser::Zen, Browser::Chromium];
        let targets = discover(&roots, &all, &[]);

        let summary: Vec<_> = targets
            .iter()
            .map(|t| (t.browser, t.name.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (Browser::Firefox, "a"),
                (Browser::Firefox, "b"),
                (Browser::Firefox, "x"),
                (Browser::Zen, "z"),
                (Browser::Chromium, "nssdb"),
            ]
        );
    }

    #[test]
    fn macos_application_support_roots_are_found() {
        let home = TempDir::new().unwrap();
        let h = home.path();
        make_profile(
            &h.join("Library/Application Support/Firefox"),
            "[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/abc.default-release\n",
            &["Profiles/abc.default-release"],
        );
        make_profile(
            &h.join("Library/Application Support/zen"),
            "[Profile0]\nName=z\nIsRelative=1\nPath=Profiles/z.Default\n",
            &["Profiles/z.Default"],
        );
        let roots = Roots::new(h, None);

        let targets = discover(&roots, &[Browser::Firefox, Browser::Zen], &[]);

        let summary: Vec<_> = targets
            .iter()
            .map(|t| (t.browser, t.name.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![(Browser::Firefox, "default-release"), (Browser::Zen, "z")]
        );
    }

    #[test]
    fn only_the_chosen_browsers_are_scanned_and_missing_dirs_are_dropped() {
        let home = TempDir::new().unwrap();
        let h = home.path();
        make_profile(
            &h.join(".zen"),
            "[Profile0]\nName=z\nIsRelative=1\nPath=z\n[Profile1]\nName=gone\nIsRelative=1\nPath=gone\n",
            &["z"],
        );
        let roots = Roots::new(h, None);

        assert!(discover(&roots, &[Browser::Firefox], &[]).is_empty());
        assert!(discover(&roots, &[Browser::Chromium], &[]).is_empty());
        let zen = discover(&roots, &[Browser::Zen], &[]);
        assert_eq!(zen.len(), 1);
        assert_eq!(zen[0].dir, h.join(".zen/z"));
    }

    #[test]
    fn xdg_config_home_overrides_dot_config_when_absolute() {
        let home = TempDir::new().unwrap();
        let xdg = TempDir::new().unwrap();
        make_profile(
            &xdg.path().join("zen"),
            "[Profile0]\nName=z\nIsRelative=1\nPath=z\n",
            &["z"],
        );
        let roots = Roots::new(home.path(), Some(xdg.path()));
        assert_eq!(discover(&roots, &[Browser::Zen], &[]).len(), 1);
        let relative = Roots::new(home.path(), Some(Path::new("relative/dir")));
        assert_eq!(relative.xdg_config, home.path().join(".config"));
    }

    #[test]
    fn an_explicit_profile_is_added_once_even_if_discovery_found_it() {
        let home = TempDir::new().unwrap();
        let h = home.path();
        make_profile(
            &h.join(".zen"),
            "[Profile0]\nName=z\nIsRelative=1\nPath=z\n",
            &["z"],
        );
        let roots = Roots::new(h, None);
        let explicit = vec![h.join(".zen/z"), h.join("other")];

        let targets = discover(&roots, &[Browser::Zen], &explicit);

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].browser, Browser::Zen);
        assert_eq!(targets[1].browser, Browser::Custom);
        assert_eq!(targets[1].dir, h.join("other"));
    }
}
