//! Android: the Storage Access Framework tree picker, through
//! `android/PendantFolders.java` in the app's embedded dex. The picked tree
//! is a `content://` URI with a persisted read permission; it is counted
//! through `DocumentsContract`, never through a path. The tree URI is kept
//! in the app's data directory for [`remembered`](super::remembered).

use jni::objects::{JString, JValue};

use super::{kept_file, FolderError, FolderReport};
use crate::keystore::with_java;

const CLASS: &str = "com.hedronite.pendant.PendantFolders";
/// How long the picker may stay open.
const PICK_SECONDS: i32 = 300;

const CANCELLED: i32 = 1;

fn error_of(code: i32) -> FolderError {
    match code {
        CANCELLED => FolderError::Cancelled,
        -5 => FolderError::NothingKept,
        other => FolderError::Platform(format!("PendantFolders error {other}")),
    }
}

/// Calls a static `String`-returning method; `None` means it failed with
/// `lastError` set, which comes back as the error.
fn call_string(
    method: &'static str,
    signature: &'static str,
    arg: Option<String>,
) -> Result<String, FolderError> {
    let (value, code) = with_java(CLASS, |env, class| {
        let java_arg = match &arg {
            Some(text) => Some(env.new_string(text)?),
            None => None,
        };
        let args: Vec<JValue> = match &java_arg {
            Some(text) => vec![JValue::Object(text)],
            None => vec![JValue::Int(PICK_SECONDS)],
        };
        let result = env
            .call_static_method(class, method, signature, &args)?
            .l()?;
        let code = env.get_static_field(class, "lastError", "I")?.i()?;
        if result.is_null() {
            return Ok((None, code));
        }
        let text: String = env.get_string(&JString::from(result))?.into();
        Ok((Some(text), code))
    })
    .map_err(|err| FolderError::Platform(err.to_string()))?;
    value.ok_or_else(|| error_of(code))
}

fn kept(tree: String) -> Result<bool, FolderError> {
    with_java(CLASS, |env, class| {
        let text = env.new_string(&tree)?;
        env.call_static_method(
            class,
            "kept",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&text)],
        )?
        .z()
    })
    .map_err(|err| FolderError::Platform(err.to_string()))
}

/// "mdTop,mdTotal,otherFiles,folders" as a report.
fn parse_counts(text: &str) -> Result<FolderReport, FolderError> {
    let numbers: Vec<usize> = text
        .split(',')
        .map(|part| part.trim().parse::<usize>())
        .collect::<Result<_, _>>()
        .map_err(|_| FolderError::Platform("unreadable counts".into()))?;
    let [md_top, md_total, other_files, folders] = numbers[..] else {
        return Err(FolderError::Platform("unreadable counts".into()));
    };
    Ok(FolderReport {
        md_top,
        md_total,
        other_files,
        folders,
        posix_path: false,
        persisted: false,
        restored: false,
    })
}

pub(super) async fn pick() -> Result<FolderReport, FolderError> {
    let file = kept_file();
    crate::net::pair::off_thread(move || {
        let tree = call_string("pick", "(I)Ljava/lang/String;", None)?;
        let persisted = match &file {
            Some(file) => file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(file, &tree))
                .is_ok(),
            None => false,
        };
        let counts = call_string(
            "count",
            "(Ljava/lang/String;)Ljava/lang/String;",
            Some(tree),
        )?;
        Ok(FolderReport {
            persisted,
            ..parse_counts(&counts)?
        })
    })
    .await
    .unwrap_or(Err(FolderError::Platform("no worker thread".into())))
}

pub(super) async fn remembered() -> Result<FolderReport, FolderError> {
    let file = kept_file().ok_or(FolderError::NothingKept)?;
    crate::net::pair::off_thread(move || {
        let tree = std::fs::read_to_string(file).map_err(|_| FolderError::NothingKept)?;
        if !kept(tree.clone())? {
            return Err(FolderError::NothingKept);
        }
        let counts = call_string(
            "count",
            "(Ljava/lang/String;)Ljava/lang/String;",
            Some(tree),
        )?;
        Ok(FolderReport {
            persisted: true,
            restored: true,
            ..parse_counts(&counts)?
        })
    })
    .await
    .unwrap_or(Err(FolderError::Platform("no worker thread".into())))
}

#[cfg(test)]
mod tests {
    use super::parse_counts;

    #[test]
    fn counts_parse_from_the_java_line() {
        let report = parse_counts("25,30,3,1").unwrap();
        assert_eq!(
            (
                report.md_top,
                report.md_total,
                report.other_files,
                report.folders
            ),
            (25, 30, 3, 1)
        );
        assert!(!report.posix_path);
        assert!(parse_counts("1,2,3").is_err());
        assert!(parse_counts("a,b,c,d").is_err());
    }
}
