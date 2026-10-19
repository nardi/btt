//! Finds, for each train label of a bundle, the page on vagonWEB that shows the layout of the
//! train, the order of its coaches.
//!
//! The address of a train page is fixed by the operator, category, number and year of the train.
//! The numbers of the trains that have a page follow no rule, and the site answers a train
//! without a page with an empty page and the status 200. So the builder links only to the pages
//! of a dictionary of known trains. The dictionary file `data/vagonweb-trains.txt` is made from
//! the sitemaps of the site that list its train pages, by the example `vagonweb_dictionary`.
//! A build reads the file and makes no request to the site.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use btt_timetable::format::{
    BundleLayoutPage, LayoutPageIndex, LayoutPageKind, PooledString, TimetableBundle,
};
use percent_encoding::percent_decode_str;
use regex::{Regex, RegexBuilder};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use smol_str::SmolStr;

use crate::config::ConfigError;
use crate::dedup::train_number_key;

/// Addresses of the sitemaps that list the train pages of the current timetable year. The site
/// splits the list in two files because a sitemap holds at most 40,000 addresses. They are the
/// source of the dictionary file.
pub const VAGONWEB_TRAIN_SITEMAP_URLS: [&str; 2] = [
    "https://www.vagonweb.cz/sitemap-razeni.xml",
    "https://www.vagonweb.cz/sitemap-razeni2.xml",
];

/// One train page of the site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainPage {
    /// Code of the operator or country, such as "DB" or "ÖBB".
    pub operator: SmolStr,
    /// Category, such as "ICE".
    pub category: SmolStr,
    /// Train number without leading zeros.
    pub number: SmolStr,
    /// Timetable year of the page.
    pub year: u16,
}

/// Matches the `<loc>` element of a sitemap entry.
static LOCATION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<loc>([^<]*)</loc>").expect("the pattern is valid"));

/// Reads the train pages out of the text of a sitemap. Entries that are not train pages and
/// entries without all of the operator, category, number and year are skipped.
pub fn parse_sitemap(text: &str) -> Vec<TrainPage> {
    LOCATION_PATTERN
        .captures_iter(text)
        .filter_map(|captures| train_of_address(&captures[1]))
        .collect()
}

/// Reads a train page address such as
/// `https://www.vagonweb.cz/razeni/vlak.php?zeme=%C3%96BB&amp;kategorie=RJ&amp;cislo=60&amp;rok=2026`.
fn train_of_address(address: &str) -> Option<TrainPage> {
    let (path, query) = address.split_once('?')?;
    if !path.ends_with("/razeni/vlak.php") {
        return None;
    }
    let (mut operator, mut category, mut number, mut year) = (None, None, None, None);
    for pair in query.split('&') {
        // The sitemap is XML, so the separators of the query come as `&amp;`.
        let pair = pair.strip_prefix("amp;").unwrap_or(pair);
        let (name, value) = pair.split_once('=')?;
        let value = percent_decode_str(value).decode_utf8().ok()?;
        match name {
            "zeme" => operator = Some(SmolStr::new(value.as_ref())),
            "kategorie" => category = Some(SmolStr::new(value.as_ref())),
            "cislo" => number = Some(SmolStr::new(value.trim_start_matches('0'))),
            "rok" => year = value.parse().ok(),
            _ => {}
        }
    }
    Some(TrainPage {
        operator: operator.filter(|operator| !operator.is_empty())?,
        category: category.filter(|category| !category.is_empty())?,
        number: number.filter(|number| !number.is_empty())?,
        year: year?,
    })
}

/// Comment lines at the top of a dictionary file.
const DICTIONARY_HEADER: &str = "\
# The train pages of vagonWEB that the builder may link to, made from its sitemaps by the example
# `vagonweb_dictionary` of `btt-builder`. Do not edit by hand.
#
# Each line holds the trains of one operator, category and year, separated by tabs. The fields
# are the operator, the category, the year and then the train numbers. A number is written as
# `first-last` when the page of every number between them exists.
";

/// Most numbers a range in a dictionary file may span, which stops a typo from making a
/// dictionary of millions of trains.
const MAXIMUM_RANGE: u32 = 100_000;

/// Reads a field such as `120-129` as the first and last number of the range. Returns `None`
/// for anything else, including a plain number.
fn range_of_numbers(field: &str) -> Option<(u32, u32)> {
    let (first, last) = field.split_once('-')?;
    let (first, last) = (first.parse().ok()?, last.parse().ok()?);
    (first <= last).then_some((first, last))
}

/// Writes a run of consecutive numbers as one number or as a range.
fn format_run((first, last): (u32, u32)) -> String {
    if first == last {
        first.to_string()
    } else {
        format!("{first}-{last}")
    }
}

/// A page of the site and the timetable year it belongs to.
#[derive(Debug, Clone)]
struct KnownPage {
    /// Category as the site writes it, which can differ in case from the one looked up.
    category: SmolStr,
    /// Timetable year of the page.
    year: u16,
}

/// The dictionary of known trains, with every page of the site that the builder may link to.
///
/// Categories are looked up without regard to case, because feeds write `rj` where the site
/// writes `RJ`.
#[derive(Debug, Default)]
pub struct TrainDictionary {
    /// The page of each train, by operator, category in lower case and number.
    trains: FxHashMap<(SmolStr, SmolStr, SmolStr), KnownPage>,
    /// The operators that have a page for a category in lower case and a number.
    operators_by_category_and_number: FxHashMap<(SmolStr, SmolStr), Vec<SmolStr>>,
    /// The list of each operator and category in lower case that has a train page.
    category_lists: FxHashMap<(SmolStr, SmolStr), KnownPage>,
    /// Year of the list of each operator that has a train page.
    operator_lists: FxHashMap<SmolStr, u16>,
}

impl TrainDictionary {
    /// Builds the dictionary from train pages.
    pub fn new(trains: impl IntoIterator<Item = TrainPage>) -> Self {
        let mut index = Self::default();
        for train in trains {
            let category_key = SmolStr::new(train.category.to_lowercase());
            let page = KnownPage {
                category: train.category,
                year: train.year,
            };
            keep_newest(
                index
                    .category_lists
                    .entry((train.operator.clone(), category_key.clone()))
                    .or_insert_with(|| page.clone()),
                &page,
            );
            let list_year = index
                .operator_lists
                .entry(train.operator.clone())
                .or_insert(train.year);
            *list_year = (*list_year).max(train.year);
            let operators = index
                .operators_by_category_and_number
                .entry((category_key.clone(), train.number.clone()))
                .or_default();
            if !operators.contains(&train.operator) {
                operators.push(train.operator.clone());
            }
            keep_newest(
                index
                    .trains
                    .entry((train.operator, category_key, train.number))
                    .or_insert_with(|| page.clone()),
                &page,
            );
        }
        index
    }

    /// Returns the number of train pages.
    pub fn train_count(&self) -> usize {
        self.trains.len()
    }

    /// Returns every train page as operator, category, number and year, in no set order.
    pub fn trains(&self) -> impl Iterator<Item = (&str, &str, &str, u16)> + '_ {
        self.trains.iter().map(|((operator, _, number), page)| {
            (
                operator.as_str(),
                page.category.as_str(),
                number.as_str(),
                page.year,
            )
        })
    }

    /// Reads the dictionary file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or a line is malformed.
    pub fn read(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_text(&text)
    }

    /// Parses the text of a dictionary file, see [`TrainDictionary::to_text`].
    ///
    /// # Errors
    ///
    /// Returns an error when a line is malformed.
    pub fn from_text(text: &str) -> Result<Self, ConfigError> {
        let mut trains = Vec::new();
        for (position, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let malformed = |reason| ConfigError::TrainDictionary {
                line: position + 1,
                reason,
            };
            let mut fields = line.split('\t');
            let (Some(operator), Some(category), Some(year)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(malformed("expected an operator, category and year"));
            };
            if operator.is_empty() || category.is_empty() {
                return Err(malformed("the operator and category must not be empty"));
            }
            let year: u16 = year
                .parse()
                .map_err(|_| malformed("the year is no number"))?;
            for field in fields {
                let numbers: Vec<SmolStr> = match range_of_numbers(field) {
                    Some((first, last)) if last - first <= MAXIMUM_RANGE => (first..=last)
                        .map(|n| SmolStr::new(n.to_string()))
                        .collect(),
                    Some(_) => return Err(malformed("a range of numbers is too long")),
                    None => vec![SmolStr::new(field)],
                };
                trains.extend(numbers.into_iter().map(|number| TrainPage {
                    operator: SmolStr::new(operator),
                    category: SmolStr::new(category),
                    number,
                    year,
                }));
            }
        }
        Ok(Self::new(trains))
    }

    /// Writes the dictionary as the text of a dictionary file.
    ///
    /// Lines starting with `#` are comments. Every other line holds the trains of one operator,
    /// category and year, separated by tabs. The first three fields are the operator, the
    /// category and the year. Each field after that is a train number, or a range of numbers
    /// such as `120-129`. The lines and the numbers are sorted, so a new dictionary differs from
    /// an old one only where the site changed.
    pub fn to_text(&self) -> String {
        let mut groups: BTreeMap<(&str, &str, u16), Vec<&str>> = BTreeMap::new();
        for (operator, category, number, year) in self.trains() {
            // A name that reads as a range would come back as several trains.
            if range_of_numbers(number).is_some() || number.contains(['\t', '\n']) {
                continue;
            }
            groups
                .entry((operator, category, year))
                .or_default()
                .push(number);
        }
        let mut text = String::from(DICTIONARY_HEADER);
        for ((operator, category, year), mut numbers) in groups {
            // The numbers that are plain integers come first, in numeric order, so they form
            // ranges. The names come after them in text order.
            numbers.sort_by_key(|number| {
                (
                    number.parse::<u32>().ok().map_or((1_u8, 0), |n| (0, n)),
                    *number,
                )
            });
            let mut fields = vec![operator.to_owned(), category.to_owned(), year.to_string()];
            let mut run: Option<(u32, u32)> = None;
            for number in numbers {
                let integer = number.parse::<u32>().ok();
                match (integer, run) {
                    (Some(next), Some((first, last))) if last.checked_add(1) == Some(next) => {
                        run = Some((first, next));
                    }
                    (_, previous) => {
                        if let Some(previous) = previous {
                            fields.push(format_run(previous));
                        }
                        run = integer.map(|integer| (integer, integer));
                        if integer.is_none() {
                            fields.push(number.to_owned());
                        }
                    }
                }
            }
            if let Some(run) = run {
                fields.push(format_run(run));
            }
            text.push_str(&fields.join("\t"));
            text.push('\n');
        }
        text
    }

    /// Returns the best page for a train of a known operator and category, trying each of the
    /// numbers in turn.
    ///
    /// The train itself comes first, then the same category and number under another operator,
    /// when `approximate_allowed` says its numbers name one train everywhere, then the list of
    /// the category, then the list of the operator.
    fn best_page(
        &self,
        operator: &str,
        category: &str,
        numbers: &[&str],
        approximate_allowed: bool,
    ) -> Option<LayoutPageMatch> {
        let category_key = SmolStr::new(category.to_lowercase());
        for number in numbers {
            let key = (
                SmolStr::new(operator),
                category_key.clone(),
                SmolStr::new(number),
            );
            if let Some(page) = self.trains.get(&key) {
                return Some(LayoutPageMatch {
                    kind: LayoutPageKind::Train,
                    operator: SmolStr::new(operator),
                    category: page.category.clone(),
                    number: SmolStr::new(number),
                    year: page.year,
                });
            }
        }
        if approximate_allowed {
            for number in numbers {
                let key = (category_key.clone(), SmolStr::new(number));
                // Two other operators with the number leave no way to tell which is the train.
                let [other_operator] = self
                    .operators_by_category_and_number
                    .get(&key)
                    .map_or(&[][..], Vec::as_slice)
                else {
                    continue;
                };
                let page_key = (
                    other_operator.clone(),
                    category_key.clone(),
                    SmolStr::new(number),
                );
                if let Some(page) = self.trains.get(&page_key) {
                    return Some(LayoutPageMatch {
                        kind: LayoutPageKind::ApproximateTrain,
                        operator: other_operator.clone(),
                        category: page.category.clone(),
                        number: SmolStr::new(number),
                        year: page.year,
                    });
                }
            }
        }
        if let Some(page) = self
            .category_lists
            .get(&(SmolStr::new(operator), category_key))
        {
            return Some(LayoutPageMatch {
                kind: LayoutPageKind::CategoryList,
                operator: SmolStr::new(operator),
                category: page.category.clone(),
                number: SmolStr::default(),
                year: page.year,
            });
        }
        let year = *self.operator_lists.get(operator)?;
        Some(LayoutPageMatch {
            kind: LayoutPageKind::OperatorList,
            operator: SmolStr::new(operator),
            category: SmolStr::default(),
            number: SmolStr::default(),
            year,
        })
    }
}

/// Replaces `known` with `candidate` when the candidate is of a later timetable year.
fn keep_newest(known: &mut KnownPage, candidate: &KnownPage) {
    if candidate.year > known.year {
        *known = candidate.clone();
    }
}

/// A page found for a train label, with the names the site uses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayoutPageMatch {
    /// What the page is about.
    pub kind: LayoutPageKind,
    /// Code of the operator or country on the site.
    pub operator: SmolStr,
    /// Category on the site. Empty for the list of an operator.
    pub category: SmolStr,
    /// Train number on the site. Empty for lists.
    pub number: SmolStr,
    /// Timetable year of the page.
    pub year: u16,
}

/// One row of `data/vagonweb.toml`, as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    agency: String,
    category: Option<String>,
    operator: String,
    vagonweb_category: Option<String>,
}

/// The rules file as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesFile {
    #[serde(default)]
    approximate_categories: Vec<String>,
    #[serde(default)]
    rule: Vec<RuleFile>,
}

/// Names the operator and category to look for on the site, for the trains of an agency.
#[derive(Debug)]
struct LayoutRule {
    /// Pattern for the agency name.
    agency: Regex,
    /// Pattern for the category of the train, ignoring case. The rule covers all categories
    /// without it.
    category: Option<Regex>,
    /// Code of the operator on the site.
    operator: SmolStr,
    /// Category on the site, when it is not the one of the train.
    vagonweb_category: Option<SmolStr>,
}

/// The rules that tie the agencies and categories of the timetable to the names of the site.
#[derive(Debug)]
pub struct LayoutRules {
    /// Rules in the order they are tried.
    rules: Vec<LayoutRule>,
    /// Categories in lower case whose numbers name the same train in every country.
    approximate_categories: Vec<String>,
}

impl LayoutRules {
    /// Reads and checks the rules file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or parsed, or a pattern does not compile.
    pub fn read(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Parses and checks the text of a rules file.
    ///
    /// # Errors
    ///
    /// Returns an error when the text does not parse or a pattern does not compile.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let file: RulesFile = toml::from_str(text)?;
        let rules = file
            .rule
            .into_iter()
            .map(|rule| {
                let pattern_error = |source| ConfigError::LayoutRulePattern {
                    agency: rule.agency.clone(),
                    source,
                };
                Ok(LayoutRule {
                    agency: Regex::new(&rule.agency).map_err(pattern_error)?,
                    category: rule
                        .category
                        .as_deref()
                        .map(|pattern| {
                            RegexBuilder::new(pattern)
                                .case_insensitive(true)
                                .build()
                                .map_err(pattern_error)
                        })
                        .transpose()?,
                    operator: SmolStr::new(&rule.operator),
                    vagonweb_category: rule.vagonweb_category.map(SmolStr::new),
                })
            })
            .collect::<Result<_, ConfigError>>()?;
        Ok(Self {
            rules,
            approximate_categories: file
                .approximate_categories
                .iter()
                .map(|category| category.to_lowercase())
                .collect(),
        })
    }

    /// Returns the best page for a train label, or `None` when no rule covers the agency.
    pub fn find_page(
        &self,
        index: &TrainDictionary,
        category: &str,
        number: &str,
        agency: &str,
    ) -> Option<LayoutPageMatch> {
        let rule = self.rules.iter().find(|rule| {
            rule.agency.is_match(agency)
                && rule
                    .category
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_match(category))
        })?;
        let site_category = rule.vagonweb_category.as_deref().unwrap_or(category);
        let number = number.trim_start_matches('0');
        // A joined train carries the number of its first part, and the site lists the part of
        // each operator under the number without the leading digit.
        let alternative = train_number_key(number);
        let numbers: Vec<&str> = [number, alternative]
            .into_iter()
            .filter(|number| !number.is_empty())
            .fold(Vec::new(), |mut numbers, number| {
                if !numbers.contains(&number) {
                    numbers.push(number);
                }
                numbers
            });
        let approximate_allowed = self
            .approximate_categories
            .contains(&site_category.to_lowercase());
        index.best_page(&rule.operator, site_category, &numbers, approximate_allowed)
    }
}

/// How many train labels of a category got each kind of page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategoryMatches {
    /// Category of the labels, as the timetable writes it.
    pub category: String,
    /// Labels in the category.
    pub labels: usize,
    /// Labels with the page of the train itself.
    pub train: usize,
    /// Labels with the page of another operator's train.
    pub approximate: usize,
    /// Labels with the list of a category.
    pub category_list: usize,
    /// Labels with the list of an operator.
    pub operator_list: usize,
}

/// What the matching did, for the build report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutPageReport {
    /// Train pages in the sitemaps.
    pub known_trains: usize,
    /// Distinct pages that labels refer to.
    pub pages: usize,
    /// The matches by category, the categories with the most labels first.
    pub categories: Vec<CategoryMatches>,
}

/// Appends strings to the string pool of a bundle, each only once.
struct PoolAppender<'bundle> {
    /// The pool of the bundle.
    pool: &'bundle mut String,
    /// Position of each string appended so far.
    positions: FxHashMap<SmolStr, PooledString>,
}

impl PoolAppender<'_> {
    /// Returns the position of a string in the pool, appending it on first use. Returns `None`
    /// when the pool outgrows its 32-bit offsets or the string outgrows its 16-bit length.
    fn intern(&mut self, text: &str) -> Option<PooledString> {
        if let Some(position) = self.positions.get(text) {
            return Some(*position);
        }
        let position = PooledString {
            byte_offset: u32::try_from(self.pool.len()).ok()?,
            byte_length: u16::try_from(text.len()).ok()?,
        };
        self.pool.push_str(text);
        self.positions.insert(SmolStr::new(text), position);
        Some(position)
    }
}

/// Returns the text of a pooled string.
fn pooled_text(pool: &str, string: PooledString) -> &str {
    let start = string.byte_offset as usize;
    pool.get(start..start + usize::from(string.byte_length))
        .unwrap_or_default()
}

/// Sets the layout page of every train label of a bundle that has a match, and returns what it
/// did. The text of the pages goes at the end of the string pool.
pub fn attach_layout_pages(
    bundle: &mut TimetableBundle,
    rules: &LayoutRules,
    index: &TrainDictionary,
) -> LayoutPageReport {
    let matches: Vec<(String, Option<LayoutPageMatch>)> = bundle
        .train_labels
        .iter()
        .map(|label| {
            let pool = &bundle.string_pool;
            let category = pooled_text(pool, label.category);
            let page = rules.find_page(
                index,
                category,
                pooled_text(pool, label.number),
                pooled_text(pool, label.agency),
            );
            (category.to_owned(), page)
        })
        .collect();

    let mut appender = PoolAppender {
        pool: &mut bundle.string_pool,
        positions: FxHashMap::default(),
    };
    let mut page_indices: FxHashMap<LayoutPageMatch, LayoutPageIndex> = FxHashMap::default();
    let mut pages: Vec<BundleLayoutPage> = Vec::new();
    let mut by_category: FxHashMap<String, CategoryMatches> = FxHashMap::default();
    for (label, (category, page)) in bundle.train_labels.iter_mut().zip(matches) {
        let counts = by_category
            .entry(category.clone())
            .or_insert_with(|| CategoryMatches {
                category,
                ..CategoryMatches::default()
            });
        counts.labels += 1;
        let Some(page) = page else { continue };
        let page_index = if let Some(page_index) = page_indices.get(&page) {
            *page_index
        } else {
            let (Some(operator), Some(category), Some(number)) = (
                appender.intern(&page.operator),
                appender.intern(&page.category),
                appender.intern(&page.number),
            ) else {
                continue;
            };
            let Ok(next) = u32::try_from(pages.len()) else {
                continue;
            };
            pages.push(BundleLayoutPage {
                kind: page.kind,
                operator,
                category,
                number,
                year: page.year,
            });
            page_indices.insert(page.clone(), LayoutPageIndex(next));
            LayoutPageIndex(next)
        };
        label.layout_page = Some(page_index);
        match page.kind {
            LayoutPageKind::Train => counts.train += 1,
            LayoutPageKind::ApproximateTrain => counts.approximate += 1,
            LayoutPageKind::CategoryList => counts.category_list += 1,
            LayoutPageKind::OperatorList => counts.operator_list += 1,
        }
    }
    let page_count = pages.len();
    bundle.layout_pages = pages;

    let mut categories: Vec<CategoryMatches> = by_category.into_values().collect();
    categories.sort_by(|left, right| {
        right
            .labels
            .cmp(&left.labels)
            .then_with(|| left.category.cmp(&right.category))
    });
    LayoutPageReport {
        known_trains: index.train_count(),
        pages: page_count,
        categories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITEMAP: &str = r"<?xml version='1.0' encoding='UTF-8'?>
<urlset>
  <url><loc>https://www.vagonweb.cz/razeni/</loc><lastmod>2026-10-04</lastmod></url>
  <url><loc>https://www.vagonweb.cz/razeni/vlak.php?zeme=%C3%96BB&amp;kategorie=RJ&amp;cislo=0060&amp;rok=2026</loc></url>
  <url><loc>https://www.vagonweb.cz/razeni/vlak.php?zeme=DB&amp;kategorie=ICE&amp;cislo=71&amp;rok=2026</loc></url>
  <url><loc>https://www.vagonweb.cz/razeni/vlak.php?zeme=DB&amp;cislo=71&amp;rok=2026</loc></url>
  <url><loc>https://www.vagonweb.cz/razeni/vlak.php?zeme=%C3%96BB&amp;kategorie=NJ&amp;cislo=421&amp;rok=2026</loc></url>
  <url><loc>https://www.vagonweb.cz/razeni/vlak.php?zeme=DB&amp;kategorie=RE&amp;cislo=4001&amp;rok=2026</loc></url>
  <url><loc>https://www.vagonweb.cz/razeni/vlak.php?zeme=DB&amp;kategorie=EC&amp;cislo=8&amp;rok=2026</loc></url>
</urlset>";

    const RULES: &str = r#"
approximate_categories = ["EC", "RJ"]

[[rule]]
agency = '^OEBB'
operator = "ÖBB"
category = '^S$'
vagonweb_category = "SB"

[[rule]]
agency = '^OEBB'
operator = "ÖBB"

[[rule]]
agency = '^DB '
operator = "DB"

[[rule]]
agency = '^Schweizerische'
operator = "SBB"
"#;

    fn page_of(category: &str, number: &str, agency: &str) -> Option<LayoutPageMatch> {
        let index = TrainDictionary::new(parse_sitemap(SITEMAP));
        LayoutRules::parse(RULES)
            .unwrap()
            .find_page(&index, category, number, agency)
    }

    fn summary(page: Option<LayoutPageMatch>) -> Option<(LayoutPageKind, String, String, String)> {
        page.map(|page| {
            (
                page.kind,
                page.operator.to_string(),
                page.category.to_string(),
                page.number.to_string(),
            )
        })
    }

    #[test]
    fn reads_train_pages_and_skips_other_entries() {
        let trains = parse_sitemap(SITEMAP);
        let names: Vec<_> = trains
            .iter()
            .take(2)
            .map(|train| {
                (
                    train.operator.as_str(),
                    train.category.as_str(),
                    train.number.as_str(),
                )
            })
            .collect();
        assert_eq!(names, [("ÖBB", "RJ", "60"), ("DB", "ICE", "71")]);
        assert_eq!(trains.len(), 5);
        assert_eq!(trains[0].year, 2026);
    }

    #[test]
    fn matches_the_train_itself_without_regard_to_case_and_leading_zeros() {
        let page = page_of("ice", "0071", "DB Fernverkehr AG");
        assert_eq!(
            summary(page),
            Some((
                LayoutPageKind::Train,
                "DB".into(),
                "ICE".into(),
                "71".into()
            ))
        );
    }

    #[test]
    fn looks_for_the_renamed_category_of_a_rule() {
        // The site has no page for the renamed category "SB", so the list of the operator comes.
        let page = page_of("S", "1", "OEBB Personenverkehr AG");
        assert_eq!(
            summary(page),
            Some((
                LayoutPageKind::OperatorList,
                "ÖBB".into(),
                String::new(),
                String::new()
            ))
        );
    }

    #[test]
    fn matches_a_joined_train_under_the_number_without_its_leading_digit() {
        let page = page_of("NJ", "40421", "OEBB Personenverkehr AG");
        assert_eq!(
            summary(page),
            Some((
                LayoutPageKind::Train,
                "ÖBB".into(),
                "NJ".into(),
                "421".into()
            ))
        );
    }

    #[test]
    fn matches_another_operators_page_for_a_listed_category() {
        let page = page_of("EC", "8", "Schweizerische Bundesbahnen SBB");
        assert_eq!(
            summary(page),
            Some((
                LayoutPageKind::ApproximateTrain,
                "DB".into(),
                "EC".into(),
                "8".into()
            ))
        );
    }

    #[test]
    fn leaves_another_operators_page_unmatched_for_other_categories() {
        let page = page_of("RE", "4001", "Schweizerische Bundesbahnen SBB");
        assert_eq!(page, None);
    }

    #[test]
    fn falls_back_to_the_list_of_the_category_then_of_the_operator() {
        assert_eq!(
            summary(page_of("ICE", "999", "DB Fernverkehr AG")),
            Some((
                LayoutPageKind::CategoryList,
                "DB".into(),
                "ICE".into(),
                String::new()
            ))
        );
        assert_eq!(
            summary(page_of("IC", "999", "DB Fernverkehr AG")),
            Some((
                LayoutPageKind::OperatorList,
                "DB".into(),
                String::new(),
                String::new()
            ))
        );
    }

    #[test]
    fn gives_no_page_to_an_agency_without_a_rule() {
        assert_eq!(page_of("ICE", "71", "Unknown Railways"), None);
    }

    /// Returns the trains of a dictionary as sorted tuples.
    fn sorted_trains(dictionary: &TrainDictionary) -> Vec<(String, String, String, u16)> {
        let mut trains: Vec<_> = dictionary
            .trains()
            .map(|(operator, category, number, year)| {
                (operator.into(), category.into(), number.into(), year)
            })
            .collect();
        trains.sort();
        trains
    }

    #[test]
    fn writes_consecutive_numbers_as_ranges() {
        let dictionary = TrainDictionary::new(
            ["8", "9", "10", "12", "Bavorský expres", "7000"].map(|number| TrainPage {
                operator: "ČD".into(),
                category: "Ex".into(),
                number: number.into(),
                year: 2026,
            }),
        );
        let text = dictionary.to_text();
        assert!(text.ends_with("ČD\tEx\t2026\t8-10\t12\t7000\tBavorský expres\n"));
    }

    #[test]
    fn reads_back_the_dictionary_it_wrote() {
        let original = TrainDictionary::new(parse_sitemap(SITEMAP));
        let copy = TrainDictionary::from_text(&original.to_text()).unwrap();
        assert_eq!(sorted_trains(&copy), sorted_trains(&original));
    }

    #[test]
    fn reports_the_line_of_a_malformed_dictionary() {
        let result = TrainDictionary::from_text("# Header\nDB\tICE\n");
        assert!(matches!(
            result,
            Err(ConfigError::TrainDictionary { line: 2, .. })
        ));
        let result = TrainDictionary::from_text("DB\tICE\t2026\t1-9999999\n");
        assert!(matches!(
            result,
            Err(ConfigError::TrainDictionary { line: 1, .. })
        ));
    }

    #[test]
    fn reads_the_dictionary_file_of_the_repository() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vagonweb-trains.txt");
        let dictionary = TrainDictionary::read(&path).unwrap();
        assert!(dictionary.train_count() > 10_000);
    }

    #[test]
    fn rejects_a_rule_with_a_broken_pattern() {
        let result = LayoutRules::parse("[[rule]]\nagency = '('\noperator = \"DB\"\n");
        assert!(matches!(result, Err(ConfigError::LayoutRulePattern { .. })));
    }
}
