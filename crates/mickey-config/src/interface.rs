//! `[interface]`: where the switcher shows and how it orders the PCs.

use serde::Deserialize;

/// `[interface]`: how Mickey shows itself.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Interface {
    /// Where the switcher appears: a corner or `center`.
    #[serde(default = "default_switcher_position")]
    pub switcher_position: String,
    /// The switcher's cards in a column instead of a row.
    #[serde(default)]
    pub switcher_vertical: bool,
    /// PC names in the order the switcher shows them; PCs left out follow,
    /// as the layout places them. Empty: all as the layout places them.
    #[serde(default)]
    pub switcher_order: Vec<String>,
    /// Where the question about bringing big files appears (PRD 0015): a
    /// corner or `center`, as the switcher's.
    #[serde(default = "default_bring_position")]
    pub bring_position: String,
    /// Files over this many gigabytes are asked about before they come
    /// (PRD 0015); `0` asks every time.
    #[serde(default = "default_bring_over_gb")]
    pub bring_over_gb: u64,
    /// Bring suggests cleaning `Downloads/Mickey` when it holds more than
    /// this many gigabytes...
    #[serde(default = "default_clean_over_gb")]
    pub clean_over_gb: u64,
    /// ...or when less than this share of the disk would be left free.
    #[serde(default = "default_clean_below_percent")]
    pub clean_below_percent: u64,
    /// The language of Mickey's windows and tray menu: `auto` (the
    /// system's), or a code from `i18n::Lang::ALL` such as `en`, `pt-BR`, `pt-PT`.
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for Interface {
    fn default() -> Interface {
        Interface {
            switcher_position: default_switcher_position(),
            switcher_vertical: false,
            switcher_order: Vec::new(),
            bring_position: default_bring_position(),
            bring_over_gb: default_bring_over_gb(),
            clean_over_gb: default_clean_over_gb(),
            clean_below_percent: default_clean_below_percent(),
            language: default_language(),
        }
    }
}

impl Interface {
    /// Above this many bytes, files are asked about before they come.
    pub fn bring_over(&self) -> u64 {
        self.bring_over_gb.saturating_mul(1_000_000_000)
    }

    /// The language to show: the chosen one, or the system's for `auto`
    /// (English when that is none Mickey is translated into). `Err` for a
    /// code Mickey does not know.
    pub fn language(&self) -> Result<crate::i18n::Lang, String> {
        if self.language == "auto" {
            return Ok(crate::i18n::Lang::system());
        }
        crate::i18n::Lang::from_code(&self.language)
            .ok_or_else(|| format!("unknown language {:?}", self.language))
    }

    /// The switcher's cards in the order to show them: each card's name and
    /// its place in the layout (`None` if not placed). The custom order goes
    /// first; then left to right as the layout places them (top to bottom
    /// in a column); unplaced PCs last. Returns indexes into `cards`.
    pub fn switcher_sort(&self, cards: &[(&str, Option<[i32; 2]>)]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..cards.len()).collect();
        order.sort_by_key(|&index| {
            let (name, place) = cards[index];
            let custom = self.switcher_order.iter().position(|wanted| wanted == name);
            let place = place.map(|[x, y]| {
                if self.switcher_vertical {
                    (y, x)
                } else {
                    (x, y)
                }
            });
            (custom.unwrap_or(usize::MAX), place.is_none(), place, index)
        });
        order
    }
}

pub const SWITCHER_POSITIONS: [&str; 5] = [
    "center",
    "top-left",
    "top-right",
    "bottom-left",
    "bottom-right",
];

/// A notice about files arriving, out of the way.
fn default_bring_position() -> String {
    "bottom-right".into()
}

fn default_bring_over_gb() -> u64 {
    1
}

fn default_clean_over_gb() -> u64 {
    10
}

fn default_clean_below_percent() -> u64 {
    10
}

fn default_switcher_position() -> String {
    "bottom-right".into()
}

fn default_language() -> String {
    "auto".into()
}
