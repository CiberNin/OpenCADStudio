// XREF path model — lexical identity for external references (Task 2).

/// Lexical path normalization for reference identity. Never touches the
/// filesystem (canonicalize fails on missing files; NotFound is common).
/// Backslash→slash, `.`/`..` component cleanup, case-fold on Windows.
pub fn normalize_lexical(raw: &str) -> String {
    // 1. Separator unification. Pure string op — no I/O, so missing files
    // (the common NotFound case) normalize exactly like present ones.
    let slashed = raw.replace('\\', "/");

    // 2. Prefix split: drive (`C:`), UNC (`//`), or none. The prefix is
    // lowercased so `C:\x` and `c:/x` hash to the same identity.
    let bytes = slashed.as_bytes();
    let (prefix, mut rest) = if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
    {
        (slashed[..2].to_ascii_lowercase(), slashed[2..].to_string())
    } else if slashed.starts_with("//") {
        ("//".to_string(), slashed[2..].to_string())
    } else {
        (String::new(), slashed)
    };
    if prefix == "//" {
        // Collapse `///share` → `//share` so the rejoin can't triple-slash.
        rest = rest.trim_start_matches('/').to_string();
    }
    let absolute = rest.starts_with('/');

    // 3. Lexical component cleanup: drop empties/`.`, resolve `..` by
    // popping. A relative path that climbs past its root keeps the surplus
    // `..` (it still means "up"); an absolute path clamps at its root.
    let mut stack: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        } else if comp == ".." {
            if stack.pop().is_none() && !absolute {
                stack.push("..");
            }
        } else {
            stack.push(comp);
        }
    }

    // 4. Case fold: Windows filesystems are case-insensitive, so identity
    // must be too. Elsewhere case is significant and preserved.
    let mut joined = stack.join("/");
    if cfg!(windows) {
        joined = joined.to_lowercase();
    }

    // 5. Rejoin as `{prefix}{joined}`, preserving rootedness.
    if absolute {
        joined = format!("/{joined}");
    }
    format!("{prefix}{joined}")
}

/// How an XREF path is stored relative to its host drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pathtype {
    Full,
    Relative,
    None,
}

/// Why a [`Pathtype::Relative`] conversion cannot be expressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathtypeError {
    AcrossDrives,
    UnsavedHost,
}

fn normalize_display(raw: &str) -> String {
    let slashed = raw.replace('\\', "/");
    let bytes = slashed.as_bytes();
    let (prefix, mut rest) = if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
    {
        (slashed[..2].to_string(), slashed[2..].to_string())
    } else if slashed.starts_with("//") {
        ("//".to_string(), slashed[2..].to_string())
    } else {
        (String::new(), slashed)
    };
    if prefix == "//" {
        rest = rest.trim_start_matches('/').to_string();
    }
    let absolute = rest.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        } else if comp == ".." {
            if stack.pop().is_none() && !absolute {
                stack.push("..");
            }
        } else {
            stack.push(comp);
        }
    }
    let mut joined = stack.join("/");
    if absolute {
        joined = format!("/{joined}");
    }
    format!("{prefix}{joined}")
}

fn root_key(normalized: &str) -> String {
    let lower = normalized.to_ascii_lowercase();
    let b = lower.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return lower[..2].to_string();
    }
    if lower.starts_with("//") {
        let mut parts = lower[2..].split('/').filter(|s| !s.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return format!("//{server}/{share}");
        }
        return lower;
    }
    if lower.starts_with('/') {
        return "/".to_string();
    }
    String::new()
}

fn is_relative_path(normalized: &str) -> bool {
    let b = normalized.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return false;
    }
    if normalized.starts_with("//") || normalized.starts_with('/') {
        return false;
    }
    true
}

fn comps_after_root(path_norm: &str) -> Vec<&str> {
    let b = path_norm.as_bytes();
    let is_drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    let is_unc = path_norm.starts_with("//");
    if is_drive {
        path_norm[2..].split('/').filter(|s| !s.is_empty()).collect()
    } else if is_unc {
        let all: Vec<&str> = path_norm[2..].split('/').filter(|s| !s.is_empty()).collect();
        if all.len() >= 2 {
            all[2..].to_vec()
        } else {
            Vec::new()
        }
    } else {
        path_norm.split('/').filter(|s| !s.is_empty()).collect()
    }
}

fn file_name_only(path: &str) -> String {
    let disp = normalize_display(path);
    if disp.is_empty() {
        return String::new();
    }
    let mut base = disp.rsplit('/').next().unwrap_or("");
    let bb = base.as_bytes();
    if bb.len() >= 2 && bb[0].is_ascii_alphabetic() && bb[1] == b':' {
        base = &base[2..];
    }
    base.to_string()
}

/// Fallible pathtype conversion. `Full` and `None` always succeed;
/// `Relative` fails with [`PathtypeError::AcrossDrives`] when drive letters
/// or UNC hosts differ (UNC vs drive-letter counts as different), and with
/// [`PathtypeError::UnsavedHost`] when `host` has no parent directory.
/// Drive comparison is case-insensitive; all comparisons use
/// [`normalize_lexical`].
pub fn to_pathtype_result(
    path: &str,
    host: &std::path::Path,
    pathtype: Pathtype,
) -> Result<String, PathtypeError> {
    match pathtype {
        Pathtype::Full => Ok(normalize_display(path)),
        Pathtype::None => Ok(file_name_only(path)),
        Pathtype::Relative => {
            let parent = host.parent();
            let parent = match parent {
                None => return Err(PathtypeError::UnsavedHost),
                Some(p) if p.as_os_str().is_empty() => return Err(PathtypeError::UnsavedHost),
                Some(p) => p,
            };
            let host_dir_norm = normalize_lexical(&parent.to_string_lossy());
            if host_dir_norm.is_empty() {
                return Err(PathtypeError::UnsavedHost);
            }
            let target_norm = normalize_lexical(path);
            if target_norm.is_empty() {
                return Ok(String::new());
            }
            let target_root = root_key(&target_norm);
            let host_root = root_key(&host_dir_norm);
            if target_root != host_root {
                if is_relative_path(&target_norm) {
                    return Ok(normalize_display(path));
                }
                return Err(PathtypeError::AcrossDrives);
            }
            let target_comps_norm = comps_after_root(&target_norm);
            let host_comps_norm = comps_after_root(&host_dir_norm);
            let mut k = 0usize;
            while k < target_comps_norm.len()
                && k < host_comps_norm.len()
                && target_comps_norm[k] == host_comps_norm[k]
            {
                k += 1;
            }
            let target_disp = normalize_display(path);
            let target_comps_disp = comps_after_root(&target_disp);
            let mut parts: Vec<String> = Vec::new();
            for _ in 0..host_comps_norm.len().saturating_sub(k) {
                parts.push("..".to_string());
            }
            if target_comps_disp.len() == target_comps_norm.len() {
                for c in target_comps_disp.iter().skip(k) {
                    parts.push(c.to_string());
                }
            } else {
                for c in target_comps_norm.iter().skip(k) {
                    parts.push(c.to_string());
                }
            }
            if parts.is_empty() {
                return Ok(file_name_only(path));
            }
            Ok(parts.join("/"))
        }
    }
}

/// Panic-free pathtype conversion. [`PathtypeError::AcrossDrives`] and
/// [`PathtypeError::UnsavedHost`] fall back to [`Pathtype::Full`]
/// (absolute, separator-normalized) so callers always get a usable path.
pub fn to_pathtype(path: &str, host: &std::path::Path, pathtype: Pathtype) -> String {
    match to_pathtype_result(path, host, pathtype) {
        Ok(s) => s,
        Err(_) => normalize_display(path),
    }
}

/// Case-insensitive name match: `*` matches any run (including empty),
/// `?` matches exactly one char.
pub fn wildcard_match(name: &str, pattern: &str) -> bool {
    let n: Vec<char> = name.chars().flat_map(|c| c.to_lowercase()).collect();
    let p: Vec<char> = pattern.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut si = 0usize;
    let mut pi = 0usize;
    let mut star: Option<usize> = None;
    let mut match_idx = 0usize;
    while si < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[si]) {
            si += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            match_idx = si;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            match_idx += 1;
            si = match_idx;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// What kind of external file a [`ReferenceEntry`] points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    DwgXref,
    Image,
    Pdf,
}

/// Lifecycle state of a [`ReferenceEntry`].
///
/// `Stale` (file changed since load) and `Orphaned` (definition with no
/// referencing entity) are detected in Task 8 — [`collect_entries`] never
/// produces them yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefStatus {
    Loaded,
    Unloaded,
    NotFound,
    Failed,
    Stale,
    Orphaned,
}

/// How a DWG xref attaches: full re-export (`Attach`) vs local-only (`Overlay`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefType {
    Attach,
    Overlay,
}

/// One external reference in the XREF manager's unified list.
///
/// `key` is the block-record handle (DWG xref) or definition-object handle
/// (image / PDF) — stable across renames, so [`renamed`](Self::renamed) keeps
/// it. `saved_path` is the raw stored string verbatim, never synthesized;
/// `found_at` is where it actually resolved, if anywhere.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceEntry {
    pub key: u64,
    pub name: String,
    pub kind: RefKind,
    pub ref_type: RefType,
    pub status: RefStatus,
    pub size_bytes: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
    pub saved_path: String,
    pub found_at: Option<String>,
    pub loaded: bool,
}

impl ReferenceEntry {
    pub fn new(key: u64, name: impl Into<String>, kind: RefKind) -> Self {
        Self {
            key,
            name: name.into(),
            kind,
            ref_type: RefType::Attach,
            status: RefStatus::NotFound,
            size_bytes: None,
            modified: None,
            saved_path: String::new(),
            found_at: None,
            loaded: true,
        }
    }

    /// Same entry under a new name — the key is untouched.
    pub fn renamed(&self, name: impl Into<String>) -> Self {
        Self {
            key: self.key,
            name: name.into(),
            ..self.clone()
        }
    }
}

/// Bind-style symbol name for a nested xref block: AutoCAD's `BIND` inserts
/// `$0$` separators, so `PLAN` → `DETAIL` → `WALLS` reads transitively.
pub fn bind_symbol(parent: &str, child: &str, sym: &str) -> String {
    format!("{parent}$0${child}$0${sym}")
}

/// [`bind_symbol`] for the two-level case, bumping the `$N$` counter past
/// every collision in `taken` (`PLAN$0$WALLS` taken → `PLAN$1$WALLS`).
pub fn bind_symbol_taken(parent: &str, sym: &str, taken: &[impl AsRef<str>]) -> String {
    let mut n = 0u32;
    loop {
        let candidate = format!("{parent}${n}${sym}");
        if !taken.iter().any(|t| t.as_ref() == candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_lexical, to_pathtype, to_pathtype_result, wildcard_match, Pathtype, PathtypeError};
    use super::{bind_symbol, bind_symbol_taken, RefKind, ReferenceEntry};

    #[test]
    fn lexical_normalize_missing_file_no_fs_touch() {
        let a = normalize_lexical("C:\\Host\\PLAN.dwg");
        let b = normalize_lexical("c:/host/./plan.dwg");
        assert_eq!(a, b);
        let c = normalize_lexical("C:\\NoSuchDir\\Sub\\..\\REF.DWG");
        let d = normalize_lexical("c:\\nosuchdir\\ref.dwg");
        assert_eq!(c, d);
    }

    #[test]
    #[cfg(windows)]
    fn lexical_normalize_exact_forms() {
        assert_eq!(normalize_lexical("C:\\Host\\PLAN.dwg"), "c:/host/plan.dwg");
        assert_eq!(
            normalize_lexical("C:\\NoSuchDir\\Sub\\..\\REF.DWG"),
            "c:/nosuchdir/ref.dwg"
        );
        assert_eq!(
            normalize_lexical("//SERVER/Share/./File.DWG"),
            "//server/share/file.dwg"
        );
        assert_eq!(normalize_lexical("c:/a/../../b.dwg"), "c:/b.dwg");
        assert_eq!(normalize_lexical("rel/../../x.dwg"), "../x.dwg");
        assert_eq!(normalize_lexical("a//b\\\\c.dwg"), "a/b/c.dwg");
        assert_eq!(normalize_lexical("a/b/"), "a/b");
        assert_eq!(normalize_lexical(""), "");
        assert_eq!(normalize_lexical("."), "");
    }

    // Deferred byte-level roundtrip probe (SPIKE1 / Task 1 carryover):
    // temp-dir only, nothing added to the repo. Builds an in-memory document
    // carrying each reference kind, writes it out, re-reads, and asserts
    // every SPIKE1 cell survived the round trip.
    #[cfg(not(target_arch = "wasm32"))]
    fn roundtrip_probe(ext: &str) {
        use acadrust::objects::{ImageDefinition, ObjectType, UnderlayDefinition};
        use acadrust::tables::BlockRecord;
        use acadrust::CadDocument;

        let dir =
            std::path::PathBuf::from(r"C:\Users\apolius\AppData\Local\Temp\opencode\xref_probe");
        std::fs::create_dir_all(&dir).expect("probe dir");

        let mut doc = CadDocument::new();

        // Cell 1: xref block record — is_xref flag + absolute path.
        let xref_abs = dir.join("PROBE_REF.dwg").to_string_lossy().into_owned();
        let mut br = BlockRecord::new("PROBE_REF");
        br.flags.is_xref = true;
        br.xref_path = xref_abs.clone();
        doc.block_records.add(br).expect("add xref block record");

        // Cell 2: raster image definition with an absolute file path.
        let img_handle = doc.allocate_handle();
        let mut img_def =
            ImageDefinition::with_dimensions(r"C:\Probe\IMG.png", 64u32, 64u32);
        img_def.handle = img_handle;
        img_def.is_loaded = true;
        doc.objects
            .insert(img_handle, ObjectType::ImageDefinition(img_def));

        // Cell 3: PDF underlay definition with an absolute file path.
        let und_handle = doc.allocate_handle();
        let mut und_def = UnderlayDefinition::pdf(r"C:\Probe\DOC.pdf", "1");
        und_def.handle = und_handle;
        doc.objects
            .insert(und_handle, ObjectType::UnderlayDefinition(und_def));

        // Cell 4: the retain flag itself ($VISRETAIN).
        doc.header.retain_xref_visibility = true;

        let bytes = crate::io::save_to_bytes(&doc, ext, doc.version)
            .expect("probe save");
        std::fs::write(dir.join(format!("probe.{ext}")), &bytes).expect("probe write");

        let back = crate::io::load_bytes(&format!("probe.{ext}"), bytes).expect("probe reload");

        let br2 = back
            .block_records
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case("PROBE_REF"))
            .expect("xref block record round-trips");
        assert!(br2.flags.is_xref, "is_xref flag round-trips");
        assert_eq!(br2.xref_path, xref_abs, "abs xref path round-trips");

        let img_back = back
            .objects
            .values()
            .find_map(|o| match o {
                ObjectType::ImageDefinition(d) => Some(d),
                _ => None,
            })
            .expect("image definition round-trips");
        assert_eq!(img_back.file_name, r"C:\Probe\IMG.png");

        let und_back = back
            .objects
            .values()
            .find_map(|o| match o {
                ObjectType::UnderlayDefinition(d) => Some(d),
                _ => None,
            })
            .expect("underlay definition round-trips");
        assert_eq!(und_back.file_path, r"C:\Probe\DOC.pdf");

        assert!(
            back.header.retain_xref_visibility,
            "retain_xref_visibility round-trips as true"
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn probe_roundtrip_dwg() {
        roundtrip_probe("dwg");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn probe_roundtrip_dxf() {
        roundtrip_probe("dxf");
    }

    #[test]
    fn pathtype_full_relative_none_roundtrip() {
        let host = std::path::Path::new("C:/Drawings/host.dwg");
        let full = to_pathtype("C:/Drawings/refs/plan.dwg", host, Pathtype::Full);
        assert_eq!(full, "C:/Drawings/refs/plan.dwg");
        let rel = to_pathtype("C:/Drawings/refs/plan.dwg", host, Pathtype::Relative);
        assert_eq!(rel, "refs/plan.dwg");
        let none = to_pathtype("C:/Drawings/refs/plan.dwg", host, Pathtype::None);
        assert_eq!(none, "plan.dwg");
    }
    #[test]
    fn relative_across_drives_errors() {
        let host = std::path::Path::new("C:/Drawings/host.dwg");
        let err = to_pathtype_result("D:/Lib/plan.dwg", host, Pathtype::Relative);
        assert_eq!(err, Err(PathtypeError::AcrossDrives));
    }
    #[test]
    fn wildcard_name_match_case_insensitive() {
        assert!(wildcard_match("PLAN-East", "plan-*"));
        assert!(wildcard_match("A1", "a?"));
        assert!(!wildcard_match("A12", "a?"));
    }

    #[test]
    fn entry_key_stable_across_rename() {
        let a = ReferenceEntry::new(101, "PLAN", RefKind::DwgXref);
        let b = a.renamed("PLAN-EAST");
        assert_eq!(a.key, b.key);
        assert_eq!(b.name, "PLAN-EAST");
    }
    #[test]
    fn bind_chain_naming_transitive() {
        assert_eq!(bind_symbol("PLAN", "DETAIL", "WALLS"), "PLAN$0$DETAIL$0$WALLS");
        assert_eq!(bind_symbol_taken("PLAN", "WALLS", &["PLAN$0$WALLS"]), "PLAN$1$WALLS");
    }
}
