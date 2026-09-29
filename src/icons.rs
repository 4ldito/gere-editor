use gpui::{prelude::*, px, rgb, svg, AssetSource, Result, SharedString, Svg};
use std::borrow::Cow;

pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let icon: &'static [u8] = match path {
            "folder" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3z"/></svg>"#,
            "folder-open" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 9V7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v2"/><path d="M3 11h18l-2 8H5z"/></svg>"#,
            "file" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h8l5 5v13H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M14 3v6h5"/></svg>"#,
            "files" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M7 3h8l4 4v12H7z"/><path d="M15 3v5h4M4 7H3v14h13v-1"/></svg>"#,
            "git" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="4" r="2"/><circle cx="6" cy="20" r="2"/><circle cx="18" cy="7" r="2"/><path d="M6 6v12M6 14c0-4 12-2 12-5"/></svg>"#,
            "chevron-right" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>"#,
            "chevron-down" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>"#,
            "collapse-all" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="7" y="7" width="13" height="13" rx="1"/><path d="M4 16V4h12M10 13h7"/></svg>"#,
            "close" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round"><path d="M6 6 18 18M18 6 6 18"/></svg>"#,
            "search" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="m16 16 5 5"/></svg>"#,
            "settings" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2 2-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1 1.56V21h-2.8v-.09a1.7 1.7 0 0 0-1-1.56 1.7 1.7 0 0 0-1.88.34l-.06.06-2-2 .06-.06A1.7 1.7 0 0 0 7.52 15a1.7 1.7 0 0 0-1.56-1H5v-2.8h.96a1.7 1.7 0 0 0 1.56-1 1.7 1.7 0 0 0-.34-1.88l-.06-.06 2-2 .06.06a1.7 1.7 0 0 0 1.88.34 1.7 1.7 0 0 0 1-1.56V4h2.8v.09a1.7 1.7 0 0 0 1 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06 2 2-.06.06a1.7 1.7 0 0 0-.34 1.88 1.7 1.7 0 0 0 1.56 1H21V14h-.04A1.7 1.7 0 0 0 19.4 15z"/></svg>"#,
            "javascript" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round"><rect x="2" y="2" width="20" height="20" rx="2"/><path d="M11 7v9c0 2-1 3-3 3m7-4c1 2 4 2 4 0 0-2-4-1-4-4 0-2 3-3 4-1"/></svg>"#,
            "html" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m8 5-6 7 6 7m8-14 6 7-6 7m-3-17-2 20"/></svg>"#,
            "css" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round"><path d="M8 4C4 4 4 7 4 9v1c0 1-.5 2-2 2 1.5 0 2 1 2 2v1c0 2 0 5 4 5m8-16c4 0 4 3 4 5v1c0 1 .5 2 2 2-1.5 0-2 1-2 2v1c0 2 0 5-4 5"/></svg>"#,
            "json" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round"><path d="M7 4C4 4 4 6 4 9s-1 3-2 3c1 0 2 0 2 3s0 5 3 5m10-16c3 0 3 2 3 5s1 3 2 3c-1 0-2 0-2 3s0 5-3 5"/></svg>"#,
            "rust" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M8 17V7h5a3 3 0 0 1 0 6H8m4 0 4 4"/></svg>"#,
            "arrow-up" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 15 7-7 7 7"/></svg>"#,
            "arrow-down" => br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 9 7 7 7-7"/></svg>"#,
            "plus" => include_bytes!("../assets/icons/plus.svg"),
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

pub fn file_icon(path: &std::path::Path) -> Svg {
    let (name, color) = match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "js" | "mjs" | "cjs" => ("javascript", 0xe5c07b),
        "html" | "htm" => ("html", 0xe06c75),
        "css" | "less" => ("css", 0x61afef),
        "json" => ("json", 0xe5c07b),
        "rs" => ("rust", 0xd19a66),
        _ => ("file", 0xabb2bf),
    };
    icon(name, color)
}
