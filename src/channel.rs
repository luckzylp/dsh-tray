//! npm release channels of the dsh CLI.
//!
//! The npm registry publishes the package under several dist-tags
//! (`latest`, `next`, `alpha`). A channel is simply a stable name for one of
//! those tags; switching channels means installing the version that tag points
//! at.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A selectable npm release channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Stable channel (`latest` dist-tag).
    #[default]
    Latest,
    /// Preview channel (`next` dist-tag).
    Next,
    /// Experimental channel (`alpha` dist-tag).
    Alpha,
}

impl Channel {
    /// All channels in menu order.
    pub const ALL: [Channel; 3] = [Channel::Latest, Channel::Next, Channel::Alpha];

    /// The npm dist-tag backing this channel.
    pub const fn tag(self) -> &'static str {
        match self {
            Channel::Latest => "latest",
            Channel::Next => "next",
            Channel::Alpha => "alpha",
        }
    }

    /// Parse a dist-tag or a user supplied channel name.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "latest" | "stable" | "release" => Some(Channel::Latest),
            "next" | "preview" | "beta" => Some(Channel::Next),
            "alpha" | "experimental" => Some(Channel::Alpha),
            _ => None,
        }
    }

    /// Short human label (identical to the npm tag).
    pub const fn display(self) -> &'static str {
        self.tag()
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// The `dist-tags` map of the dsh npm package: tag name -> version string.
pub type DistTags = BTreeMap<String, String>;

/// Version published under `channel`, when known.
pub fn tag_version(tags: &DistTags, channel: Channel) -> Option<&str> {
    tags.get(channel.tag()).map(String::as_str)
}

/// Compact `latest=0.2.1-rc.5 next=... alpha=...` rendering for logs.
pub fn tags_summary(tags: &DistTags) -> String {
    Channel::ALL
        .iter()
        .map(|channel| {
            let version = tag_version(tags, *channel).unwrap_or("?");
            format!("{}={}", channel.tag(), version)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_roundtrip() {
        for channel in Channel::ALL {
            assert_eq!(Channel::parse(channel.tag()), Some(channel));
            assert_eq!(Channel::parse(&channel.tag().to_uppercase()), Some(channel));
        }
    }

    #[test]
    fn aliases() {
        assert_eq!(Channel::parse("stable"), Some(Channel::Latest));
        assert_eq!(Channel::parse("preview"), Some(Channel::Next));
        assert_eq!(Channel::parse("experimental"), Some(Channel::Alpha));
        assert_eq!(Channel::parse("nightly"), None);
    }

    #[test]
    fn readings_tags() {
        let mut tags = DistTags::new();
        tags.insert("latest".into(), "0.2.1-rc.5".into());
        tags.insert("alpha".into(), "0.2.2-alpha.2".into());
        assert_eq!(tag_version(&tags, Channel::Latest), Some("0.2.1-rc.5"));
        assert_eq!(tag_version(&tags, Channel::Next), None);
        assert_eq!(tags_summary(&tags), "latest=0.2.1-rc.5 next=? alpha=0.2.2-alpha.2");
    }
}
