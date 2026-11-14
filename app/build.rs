//! Android only: compiles `android/PendantKey.java` to dex, which
//! `platform/keystore.rs` embeds and loads with `InMemoryDexClassLoader`.
//!
//! For every other target this does nothing, so a host build (CI on Linux,
//! with no JDK or Android SDK) is unchanged. On Android it needs a JDK
//! (`JAVA_HOME`, else `javac` on the path) and the SDK (`ANDROID_HOME`, else
//! `ANDROID_SDK_ROOT`) with a platform and build-tools: the ones `nsh package
//! --target android` already uses. No dex, class or jar is committed.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The platform the Java compiles against: the app's compile SDK.
const PLATFORM: &str = "android-35";
/// Android 8, the oldest the app runs on (`InMemoryDexClassLoader` needs 26).
const MIN_API: &str = "26";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        return;
    }
    println!("cargo:rerun-if-changed=android/PendantKey.java");
    for var in ["JAVA_HOME", "ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let sdk = env::var_os("ANDROID_HOME")
        .or_else(|| env::var_os("ANDROID_SDK_ROOT"))
        .map(PathBuf::from)
        .expect("an Android build needs ANDROID_HOME (or ANDROID_SDK_ROOT)");
    let android_jar = sdk.join("platforms").join(PLATFORM).join("android.jar");
    assert!(
        android_jar.is_file(),
        "missing {}: install platforms;{PLATFORM}",
        android_jar.display()
    );
    let d8 = newest_build_tools(&sdk).join("d8");
    let javac = env::var_os("JAVA_HOME")
        .map(|home| PathBuf::from(home).join("bin").join("javac"))
        .unwrap_or_else(|| PathBuf::from("javac"));

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let classes = out.join("pendant-classes");
    let dex = out.join("pendant-dex");
    for dir in [&classes, &dex] {
        let _ = fs::remove_dir_all(dir);
        fs::create_dir_all(dir).expect("create a build directory");
    }

    run(Command::new(&javac)
        .args(["--release", "8", "-Xlint:-options", "-encoding", "UTF-8"])
        .arg("-classpath")
        .arg(&android_jar)
        .arg("-d")
        .arg(&classes)
        .arg("android/PendantKey.java"));

    let mut class_files = Vec::new();
    collect_classes(&classes, &mut class_files);
    class_files.sort();
    assert!(!class_files.is_empty(), "javac wrote no classes");
    run(Command::new(&d8)
        .args(["--release", "--min-api", MIN_API])
        .arg("--lib")
        .arg(&android_jar)
        .arg("--output")
        .arg(&dex)
        .args(&class_files));

    let classes_dex = dex.join("classes.dex");
    assert!(classes_dex.is_file(), "d8 wrote no classes.dex");
    println!("cargo:rustc-env=PENDANT_KEY_DEX={}", classes_dex.display());
}

fn newest_build_tools(sdk: &Path) -> PathBuf {
    let root = sdk.join("build-tools");
    let mut versions: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap_or_else(|err| panic!("read {}: {err}", root.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join("d8").is_file())
        .collect();
    versions.sort();
    versions
        .pop()
        .unwrap_or_else(|| panic!("no build-tools with d8 under {}", root.display()))
}

fn collect_classes(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir)
        .expect("read the class directory")
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            collect_classes(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "class") {
            out.push(path);
        }
    }
}

fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("run {command:?}: {err}"));
    assert!(status.success(), "{command:?} failed: {status}");
}
