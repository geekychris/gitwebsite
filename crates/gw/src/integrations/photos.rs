//! Apple Photos (macOS): list albums, and export the photos selected in Photos or an album's
//! photos to a folder, by scripting the Photos app with `osascript`. Exports are the current
//! versions (edits applied) as JPEG; videos are exported too and left for the caller to skip.
//! The first use asks for permission to control Photos (System Settings → Privacy & Security →
//! Automation).

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Album {
    /// Folder path and name, e.g. "Trips / Iceland".
    pub name: String,
    pub id: String,
    pub count: usize,
}

const LIST_ALBUMS: &str = r#"on walk(c, prefix)
	tell application "Photos"
		set out to ""
		repeat with a in (get albums of c)
			set out to out & prefix & (name of a) & tab & (id of a) & tab & ((count of media items of a) as text) & linefeed
		end repeat
		repeat with f in (get folders of c)
			set out to out & my walk(f, prefix & (name of f) & " / ")
		end repeat
		return out
	end tell
end walk
return walk(application "Photos", "")"#;

// argv: destination folder, then "selection" or "album" <id> <limit>.
const EXPORT: &str = r#"on run argv
	set dest to POSIX file (item 1 of argv)
	tell application "Photos"
		with timeout of 3600 seconds
			if item 2 of argv is "selection" then
				set picked to (get selection)
				if (count of picked) = 0 then error "Nothing is selected in Photos: select some photos there first." number 1001
			else
				set picked to (get media items of album id (item 3 of argv))
				set n to (item 4 of argv) as integer
				if n > 0 and (count of picked) > n then set picked to items 1 thru n of picked
				if (count of picked) = 0 then error "That album is empty." number 1002
			end if
			export picked to dest without using originals
		end timeout
	end tell
	return (count of picked) as text
end run"#;

fn osascript(script: &str, args: &[&str]) -> Result<String> {
    if !cfg!(target_os = "macos") {
        bail!("Apple Photos is only available on macOS");
    }
    let out = Command::new("osascript").arg("-e").arg(script).args(args).output().context("running osascript")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-1743") || err.contains("Not authorized") {
            bail!("gitwebsite isn't allowed to control Photos: allow it in System Settings → Privacy & Security → Automation, then try again");
        }
        // osascript errors look like "…: execution error: Nothing is selected in Photos… (1001)".
        let msg = err.split("execution error: ").nth(1).unwrap_or(&err).trim();
        let msg = msg.rsplit_once(" (").map(|(m, _)| m).unwrap_or(msg);
        bail!("Photos: {}", msg.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Parse the album list: one `name<TAB>id<TAB>count` per line.
pub fn parse_albums(text: &str) -> Vec<Album> {
    text.lines()
        .filter_map(|l| {
            let mut p = l.split('\t');
            let (name, id, count) = (p.next()?, p.next()?, p.next()?);
            Some(Album { name: name.to_string(), id: id.trim().to_string(), count: count.trim().parse().unwrap_or(0) })
        })
        .filter(|a| !a.id.is_empty())
        .collect()
}

/// Every album in the Photos library (albums inside folders too), in Photos' order.
pub fn albums() -> Result<Vec<Album>> {
    Ok(parse_albums(&osascript(LIST_ALBUMS, &[])?))
}

/// What to export.
pub enum Pick<'a> {
    /// The photos selected in the Photos app now.
    Selection,
    /// An album by id (from [`albums`]); at most `limit` items (0 = all).
    Album { id: &'a str, limit: usize },
}

/// Export into `dest` (created if needed); returns how many items Photos exported.
pub fn export(pick: &Pick, dest: &Path) -> Result<usize> {
    std::fs::create_dir_all(dest)?;
    let d = dest.display().to_string();
    let n = match pick {
        Pick::Selection => osascript(EXPORT, &[&d, "selection"])?,
        Pick::Album { id, limit } => {
            if id.is_empty() || id.contains('"') {
                bail!("bad album id");
            }
            osascript(EXPORT, &[&d, "album", id, &limit.to_string()])?
        }
    };
    Ok(n.trim().parse().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn albums_parse() {
        let a = parse_albums("SF House\tAB-1/L0/040\t100\nTrips / Iceland\tCD-2/L0/040\t7\n\nbroken line\n");
        assert_eq!(a, vec![
            Album { name: "SF House".into(), id: "AB-1/L0/040".into(), count: 100 },
            Album { name: "Trips / Iceland".into(), id: "CD-2/L0/040".into(), count: 7 },
        ]);
    }
}
