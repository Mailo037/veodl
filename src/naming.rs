use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{optional, Error, Result};
use crate::metadata::{first, positive, Metadata};
use crate::options::Options;
use crate::paths;
use crate::utils::{js_trim, sanitize_title};

const FIELDS: [&str; 6] = ["title", "id", "channel", "year", "playlist", "index"];

pub fn validate_template(template: &str, folders: bool) -> Result<&str> {
    if js_trim(template).is_empty() {
        return Err(Error::msg("A naming template cannot be empty."));
    }
    if template.starts_with('/') || template.chars().any(|c| c == ':' || c == '\\' || (c as u32) < 0x20) {
        return Err(Error::msg("Templates must be relative and use / for subfolders."));
    }
    if !folders && template.contains('/') {
        return Err(Error::msg("Use --folder-template for subfolders."));
    }
    if template.split('/').any(|part| matches!(js_trim(part), "" | "." | "..")) {
        return Err(Error::msg("Templates cannot contain empty, . or .. path components."));
    }
    for captures in re!(r"\{([^{}]+)\}").captures_iter(template) {
        if !FIELDS.contains(&&captures[1]) {
            let known = FIELDS.iter().map(|field| format!("{{{field}}}")).collect::<Vec<_>>().join(", ");
            return Err(Error::msg(format!("Unknown template field: {}. Use {known}.", &captures[1])));
        }
    }
    let rest = re!(r"\{([^{}]+)\}").replace_all(template, "");
    if rest.contains('{') || rest.contains('}') {
        return Err(Error::msg("Unbalanced braces in naming template."));
    }
    Ok(template)
}

pub struct Destination {
    pub directory: PathBuf,
    pub title: String,
}

pub fn media_destination(options: &Options, metadata: &Metadata, fallback_title: &str) -> Result<Destination> {
    if options.neutral_filename {
        return Ok(Destination { directory: paths::resolve(&options.output), title: format!("video-{}", hex::encode(crate::state::random_bytes::<6>())) });
    }
    let index = options.entry_index.or(positive(metadata.playlist_index).map(|index| index as u64)).unwrap_or(1);
    let year = metadata.upload_date.as_deref().and_then(|date| re!(r"^[0-9]{4}").find(date)).map(|found| found.as_str().to_string());
    let field = |name: &str| -> String {
        match name {
            "title" => first(&[&metadata.title, &metadata.id]).unwrap_or("video").to_string(),
            "id" => first(&[&metadata.id]).unwrap_or("unknown").to_string(),
            "channel" => first(&[&metadata.channel, &metadata.uploader]).unwrap_or("Unknown channel").to_string(),
            "year" => year.clone().unwrap_or_else(|| "Unknown year".into()),
            "playlist" => first(&[&options.playlist_title, &metadata.playlist_title, &metadata.playlist]).unwrap_or("No playlist").to_string(),
            "index" => format!("{index:03}"),
            _ => String::new(),
        }
    };
    let expand = |template: &str| re!(r"\{([^{}]+)\}").replace_all(template, |captures: &regex::Captures| sanitize_title(&field(&captures[1]))).into_owned();
    let title = match &options.filename_template {
        Some(template) => sanitize_title(&expand(validate_template(template, false)?)),
        None => fallback_title.to_string(),
    };
    let mut directory = paths::resolve(&options.output);
    if let Some(template) = &options.folder_template {
        for part in validate_template(template, true)?.split('/') {
            directory.push(sanitize_title(&expand(part)));
        }
    }
    Ok(Destination { directory: paths::normalize(&directory), title })
}

/// Refuse symlink/junction subfolders: metadata must never redirect a save outside output.
pub fn prepare_destination(root: &Path, directory: &Path, create: bool) -> Result<()> {
    let root = paths::resolve(root);
    let directory = paths::resolve(directory);
    let Ok(relative) = directory.strip_prefix(&root) else {
        return Err(Error::msg("Destination escapes output directory."));
    };
    if create {
        fs::create_dir_all(&root)?;
    }
    let mut current = root.clone();
    for component in relative.components() {
        current.push(component.as_os_str());
        let info = optional(fs::symlink_metadata(&current))?;
        if let Some(info) = &info {
            if !info.is_dir() || info.file_type().is_symlink() {
                return Err(Error::msg(format!("Output subfolder is not a plain directory: {}", current.display())));
            }
        }
        if info.is_none() && create {
            if let Err(error) = fs::create_dir(&current) {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error.into());
                }
            }
            let created = fs::symlink_metadata(&current)?;
            if !created.is_dir() || created.file_type().is_symlink() {
                return Err(Error::msg(format!("Output subfolder is not a plain directory: {}", current.display())));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_templates() {
        assert!(validate_template("{index} - {title}", false).is_ok());
        assert!(validate_template("{channel}/{year}", true).is_ok());
        assert!(validate_template("{channel}/{year}", false).is_err());
        assert!(validate_template("/abs", true).is_err());
        assert!(validate_template("a/../b", true).is_err());
        assert!(validate_template("{nope}", false).unwrap_err().message.starts_with("Unknown template field: nope."));
        assert!(validate_template("{title", false).is_err());
    }

    #[test]
    fn expands_templates() {
        let options = Options {
            output: "out".into(),
            filename_template: Some("{index} - {title}".into()),
            folder_template: Some("{channel}/{year}".into()),
            entry_index: Some(7),
            ..Options::default()
        };
        let metadata = Metadata { title: Some("A/B".into()), uploader: Some("Up".into()), upload_date: Some("20240102".into()), ..Metadata::default() };
        let destination = media_destination(&options, &metadata, "fallback").unwrap();
        assert_eq!(destination.title, "007 - A_B");
        assert!(destination.directory.ends_with(Path::new("out").join("Up").join("2024")));
    }
}
