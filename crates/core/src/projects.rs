//! Projects: the archives and lists you work with, as GitHub has
//! repositories. The Linux kernel on lore, U-Boot on lore too, GCC on
//! sourceware: each a project of its own, with its own lists.
//!
//! A project is a setting (`projects`): a name, a public-inbox archive, the
//! inboxes that are the project's on it (none: every one), and the
//! recipients that put a posting in it when it came from your own mailbox
//! rather than a search. Which projects a series is in is derived, never
//! stored: where its thread was found, the lists it went to, its To and Cc.

use crate::rules::glob;
use crate::series::{Facts, Series};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Project {
    pub name: String,
    /// The archive's base URL, without a trailing "/".
    pub archive: String,
    /// The project's inboxes on the archive; none is every one.
    pub lists: Vec<String>,
    /// Globs on a posting's To and Cc (lower case), for mail no search found.
    pub recipients: Vec<String>,
}

/// Projects anyone can add by name: (name, archive, lists, recipients). The
/// kernel has hundreds of lists, so it goes by its lists' hosts instead.
pub const PRESETS: &[(&str, &str, &[&str], &[&str])] = &[
    (
        "Linux",
        "https://lore.kernel.org",
        &[],
        &[
            "*@vger.kernel.org",
            "*@lists.infradead.org",
            "*@lists.linux.dev",
            "*@lists.ozlabs.org",
            "*@kvack.org",
            "*@lists.freedesktop.org",
        ],
    ),
    ("U-Boot", "https://lore.kernel.org", &["u-boot"], &["u-boot@lists.denx.de"]),
    ("QEMU", "https://lore.kernel.org", &["qemu-devel"], &["qemu-devel@nongnu.org"]),
    ("Git", "https://lore.kernel.org", &["git"], &["git@vger.kernel.org"]),
    ("Xen", "https://lore.kernel.org", &["xen-devel"], &["xen-devel@lists.xenproject.org"]),
    ("buildroot", "https://lore.kernel.org", &["buildroot"], &["buildroot@buildroot.org"]),
    ("GCC", "https://inbox.sourceware.org", &["gcc-patches"], &["gcc-patches@gcc.gnu.org"]),
    ("glibc", "https://inbox.sourceware.org", &["libc-alpha"], &["libc-alpha@sourceware.org"]),
];

/// A preset, by name in any case.
pub fn preset(name: &str) -> Option<Project> {
    PRESETS.iter().find(|p| p.0.eq_ignore_ascii_case(name)).map(|(name, archive, lists, recipients)| Project {
        name: name.to_string(),
        archive: archive.to_string(),
        lists: lists.iter().map(|s| s.to_string()).collect(),
        recipients: recipients.iter().map(|s| s.to_string()).collect(),
    })
}

/// The projects a series is in, by name. `default_archive` is what a
/// subscription with no archive of its own searched.
///
/// A project claims a series that a sync of its archive found on one of
/// its lists, or that was sent to a recipient it names; a project with
/// neither lists nor recipients, anything found on its archive. A claim by
/// a list or by an exact address beats one by a pattern, so a series to
/// git@vger.kernel.org is Git's, not also Linux's for "*@vger.kernel.org".
pub fn of<'a>(s: &Series, facts: &Facts, projects: &'a [Project], default_archive: &str) -> Vec<&'a str> {
    let found_on = |archive: &str| {
        facts.archives.get(&s.root).is_some_and(|keys| {
            keys.iter().any(|k| if k.is_empty() { default_archive == archive } else { k == archive })
        })
    };
    let mut exact = vec![];
    let mut loose = vec![];
    for p in projects {
        let by_list = !p.lists.is_empty() && found_on(&p.archive) && p.lists.iter().any(|l| s.lists.contains(l));
        let exact_rcpt = p.recipients.iter().any(|g| !g.contains(['*', '?', '[']) && s.to.contains(g));
        if by_list || exact_rcpt {
            exact.push(p.name.as_str());
        } else if p.recipients.iter().any(|g| s.to.iter().any(|a| glob(g, a)))
            || (p.lists.is_empty() && p.recipients.is_empty() && found_on(&p.archive))
        {
            loose.push(p.name.as_str());
        }
    }
    if exact.is_empty() { loose } else { exact }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(root: &str, to: &[&str], lists: &[&str]) -> Series {
        let raw = format!(
            "Message-ID: <{root}>\nFrom: d@x.org\nTo: {}\nSubject: [PATCH] a: b\nDate: Mon, 05 Oct 2026 10:00:00 +0000\n\n---\n",
            to.join(", ")
        );
        let msgs = [crate::Message::parse(raw.as_bytes()).unwrap()];
        let mut facts = Facts::default();
        facts.lists.insert(root.into(), lists.iter().map(|l| l.to_string()).collect());
        crate::series::build(&msgs, &facts, &Default::default()).remove(root).unwrap()
    }

    #[test]
    fn a_series_is_in_the_projects_that_claim_it() {
        let projects: Vec<Project> = ["Linux", "Git", "GCC"].iter().map(|n| preset(n).unwrap()).collect();
        let mut facts = Facts::default();
        let names = |s: &Series, facts: &Facts| of(s, facts, &projects, "https://lore.kernel.org");
        // From your mailbox: by its recipients; the exact address beats the pattern.
        assert_eq!(names(&series("a@x", &["linux-usb@vger.kernel.org"], &[]), &facts), ["Linux"]);
        assert_eq!(names(&series("b@x", &["git@vger.kernel.org"], &[]), &facts), ["Git"]);
        assert!(names(&series("c@x", &["someone@example.org"], &[]), &facts).is_empty());
        // Found by a search: on the project's archive and list. A default-archive sync counts as lore.
        facts.archives.insert("d@x".into(), [String::new()].into());
        assert_eq!(names(&series("d@x", &["d@x.org"], &["git"]), &facts), ["Git"]);
        facts.archives.insert("e@x".into(), ["https://inbox.sourceware.org".to_string()].into());
        assert_eq!(names(&series("e@x", &["d@x.org"], &["gcc-patches"]), &facts), ["GCC"]);
        // The same list name on another archive is not the project's.
        assert!(names(&series("e@x", &["d@x.org"], &["git"]), &facts).is_empty());
        assert!(preset("gcc").is_some() && preset("nope").is_none());
    }
}
