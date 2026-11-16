//! Local Linux camera inventory. Discovery does not open or reserve devices.
use std::path::{Path, PathBuf};

#[derive(serde::Serialize)]
pub struct CaptureDevice {
    pub name: String,
    pub device: String,
}

pub fn devices() -> Vec<CaptureDevice> {
    discover(Path::new("/sys/class/video4linux"), Path::new("/dev"))
}

fn discover(sys: &Path, dev: &Path) -> Vec<CaptureDevice> {
    let Ok(entries) = std::fs::read_dir(sys) else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let node = entry.file_name().to_string_lossy().into_owned();
        if !node
            .strip_prefix("video")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let path = dev.join(&node);
        let mut stable: Vec<PathBuf> = ["v4l/by-id", "v4l/by-path"]
            .iter()
            .flat_map(|dir| {
                std::fs::read_dir(dev.join(dir))
                    .into_iter()
                    .flatten()
                    .flatten()
            })
            .filter_map(|entry| {
                (entry.path().canonicalize().ok().as_ref() == Some(&path)).then(|| entry.path())
            })
            .collect();
        stable.sort();
        let device = stable
            .first()
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let name = std::fs::read_to_string(entry.path().join("name"))
            .unwrap_or_else(|_| node.clone())
            .trim()
            .to_string();
        devices.push(CaptureDevice {
            name: format!("{name} ({node})"),
            device,
        });
    }
    devices.sort_by(|a, b| a.device.cmp(&b.device));
    devices
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn discovery_prefers_stable_device_paths_and_keeps_inputs_separate() {
        let root = tempfile::tempdir().unwrap();
        let sys = root.path().join("sys");
        let dev = root.path().join("dev");
        std::fs::create_dir_all(dev.join("v4l/by-id")).unwrap();
        for index in 0..4 {
            let node = format!("video{index}");
            std::fs::create_dir_all(sys.join(&node)).unwrap();
            std::fs::write(sys.join(&node).join("name"), format!("HDMI {index}\n")).unwrap();
            std::fs::write(dev.join(&node), "").unwrap();
            std::os::unix::fs::symlink(
                dev.join(&node),
                dev.join("v4l/by-id").join(format!("usb-hdmi-{index}")),
            )
            .unwrap();
        }
        let devices = discover(&sys, &dev);
        assert_eq!(devices.len(), 4);
        assert!(devices.iter().all(|d| d.device.contains("by-id/usb-hdmi-")));
        assert!(devices[0].name.contains("HDMI 0"));
        assert!(discover(&root.path().join("absent"), &dev).is_empty());
    }
}
