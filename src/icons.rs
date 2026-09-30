use gpui::{prelude::*, px, rgb, svg, AssetSource, Result, SharedString, Svg};
use std::borrow::Cow;

pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let icon: &'static [u8] = match path {
            "folder" => include_bytes!("../assets/icons/folder.svg"),
            "folder-open" => include_bytes!("../assets/icons/folder-open.svg"),
            "folder-plus" => include_bytes!("../assets/icons/folder-plus.svg"),
            "file" => include_bytes!("../assets/icons/file.svg"),
            "files" => include_bytes!("../assets/icons/files.svg"),
            "file-plus-corner" => include_bytes!("../assets/icons/file-plus-corner.svg"),
            "file-code" => include_bytes!("../assets/icons/file-code.svg"),
            "file-lock" => include_bytes!("../assets/icons/file-lock.svg"),
            "file-text" => include_bytes!("../assets/icons/file-text.svg"),
            "file-cog" => include_bytes!("../assets/icons/file-cog.svg"),
            "bot" => include_bytes!("../assets/icons/bot.svg"),
            "git" => include_bytes!("../assets/icons/git-branch.svg"),
            "chevron-right" => include_bytes!("../assets/icons/chevron-right.svg"),
            "chevron-down" => include_bytes!("../assets/icons/chevron-down.svg"),
            "collapse-all" => include_bytes!("../assets/icons/square-minus.svg"),
            "close" => include_bytes!("../assets/icons/x.svg"),
            "search" => include_bytes!("../assets/icons/search.svg"),
            "case-sensitive" => include_bytes!("../assets/icons/case-sensitive.svg"),
            "whole-word" => include_bytes!("../assets/icons/whole-word.svg"),
            "regex" => include_bytes!("../assets/icons/regex.svg"),
            "eye" => include_bytes!("../assets/icons/eye.svg"),
            "settings" => include_bytes!("../assets/icons/settings.svg"),
            "javascript" | "rust" => include_bytes!("../assets/icons/file-code.svg"),
            "html" => include_bytes!("../assets/icons/code-xml.svg"),
            "svg" => include_bytes!("../assets/icons/image.svg"),
            "css" | "json" => include_bytes!("../assets/icons/braces.svg"),
            "arrow-up" => include_bytes!("../assets/icons/arrow-up.svg"),
            "arrow-down" => include_bytes!("../assets/icons/arrow-down.svg"),
            "refresh-cw" => include_bytes!("../assets/icons/refresh-cw.svg"),
            "plus" => include_bytes!("../assets/icons/plus.svg"),
            "check" => include_bytes!("../assets/icons/check.svg"),
            "minus" => include_bytes!("../assets/icons/minus.svg"),
            "trash" => include_bytes!("../assets/icons/trash.svg"),
            "maximize" => include_bytes!("../assets/icons/maximize.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(icon)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

pub fn icon(name: &'static str, color: u32) -> Svg {
    svg().path(name).size(px(16.)).text_color(rgb(color))
}

pub fn file_icon_name(path: &std::path::Path) -> &'static str {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match filename {
        ".gitignore" | ".gitattributes" | ".gitmodules" => "git",
        "AGENTS.md" | "CLAUDE.md" => "bot",
        "Cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" => "file-lock",
        "Cargo.toml" | "package.json" => "file-cog",
        _ => match extension.as_str() {
            "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => "javascript",
            "html" | "htm" => "html",
            "svg" => "svg",
            "css" | "less" | "scss" => "css",
            "json" => "json",
            "rs" => "rust",
            "md" | "txt" => "file-text",
            _ => "file",
        },
    }
}

pub fn file_icon(path: &std::path::Path) -> Svg {
    let name = file_icon_name(path);
    let color = match name {
        "git" | "file-lock" => 0x8b98a9,
        "bot" => 0x61afef,
        "file-cog" | "rust" => 0xd19a66,
        "javascript" | "json" => 0xe5c07b,
        "html" => 0xe06c75,
        "svg" => 0xc678dd,
        "css" => 0x61afef,
        "file-text" => 0x8dbad1,
        _ => 0xabb2bf,
    };
    icon(name, color)
}
