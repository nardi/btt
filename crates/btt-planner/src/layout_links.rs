//! Links from planned rides to the pages that show the layout of their trains.
//!
//! The bundle stores which page of vagonWEB belongs to each train, as the codes the site uses.
//! This module turns those codes into addresses for a ride on a given day.

use btt_model::{LayoutLink, LayoutLinkKind, LayoutLinkSource, UnixTimeMinutes};
use btt_timetable::{LayoutPage, LayoutPageKind};
use chrono::{Datelike, NaiveDate, Weekday};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// The address of the vagonWEB pages about train layouts.
const VAGONWEB_LAYOUT_URL: &str = "https://www.vagonweb.cz/razeni/";

/// The characters of a query value that stay as they are. Everything else is percent-encoded,
/// including letters such as "Ö" and "Č" that operator codes contain.
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Returns the timetable year a day belongs to. The timetable year starts on the second Sunday
/// of December and is named after the calendar year it mostly lies in, so 13 December 2026 is
/// the first day of 2027.
pub(crate) fn timetable_year(date: NaiveDate) -> i32 {
    let december_first = NaiveDate::from_ymd_opt(date.year(), 12, 1)
        .expect("the first of December exists in every year");
    // The first Sunday is at most six days after the first, and the second is a week later.
    let days_to_first_sunday = (7 - december_first.weekday().num_days_from_sunday()) % 7;
    let second_sunday = december_first + chrono::Days::new(u64::from(days_to_first_sunday) + 7);
    debug_assert_eq!(second_sunday.weekday(), Weekday::Sun);
    if date >= second_sunday {
        date.year() + 1
    } else {
        date.year()
    }
}

/// Returns the calendar day, in UTC, of a time. A ride that leaves close to midnight can fall on
/// the other side of the year change in local time, which only matters for one night a year.
fn utc_date(time: UnixTimeMinutes) -> Option<NaiveDate> {
    chrono::DateTime::from_timestamp(i64::from(time.0) * 60, 0).map(|moment| moment.date_naive())
}

fn encode(value: &str) -> String {
    utf8_percent_encode(value, QUERY_VALUE).to_string()
}

/// Returns the address of a vagonWEB page.
fn vagonweb_url(page: &LayoutPage<'_>) -> String {
    let operator = encode(page.operator);
    let year = page.year;
    match page.kind {
        LayoutPageKind::Train | LayoutPageKind::ApproximateTrain => format!(
            "{VAGONWEB_LAYOUT_URL}vlak.php?zeme={operator}&kategorie={}&cislo={}&rok={year}&lang=en",
            encode(page.category),
            encode(page.number),
        ),
        LayoutPageKind::CategoryList => format!(
            "{VAGONWEB_LAYOUT_URL}razeni.php?zeme={operator}&kategorie={}&rok={year}&lang=en",
            encode(page.category),
        ),
        LayoutPageKind::OperatorList => {
            format!("{VAGONWEB_LAYOUT_URL}razeni.php?zeme={operator}&rok={year}&lang=en")
        }
    }
}

fn link_kind(kind: LayoutPageKind) -> LayoutLinkKind {
    match kind {
        LayoutPageKind::Train => LayoutLinkKind::Train,
        LayoutPageKind::ApproximateTrain => LayoutLinkKind::ApproximateTrain,
        LayoutPageKind::CategoryList => LayoutLinkKind::CategoryList,
        LayoutPageKind::OperatorList => LayoutLinkKind::OperatorList,
    }
}

/// Returns the links for a ride that departs at `departure` and has the given page in the
/// bundle. The site keeps one set of pages per timetable year, so a ride in another year than
/// the page's gets no link. Its train may run differently by then.
pub(crate) fn layout_links(
    page: Option<LayoutPage<'_>>,
    departure: UnixTimeMinutes,
) -> Vec<LayoutLink> {
    let Some(page) = page else {
        return Vec::new();
    };
    let Some(date) = utc_date(departure) else {
        return Vec::new();
    };
    if i32::from(page.year) != timetable_year(date) {
        return Vec::new();
    }
    vec![LayoutLink {
        source: LayoutLinkSource::Vagonweb,
        kind: link_kind(page.kind),
        url: vagonweb_url(&page),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(
        kind: LayoutPageKind,
        operator: &'static str,
        category: &'static str,
    ) -> LayoutPage<'static> {
        LayoutPage {
            kind,
            operator,
            category,
            number: "71",
            year: 2026,
        }
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// Minutes since the epoch at noon UTC of a day.
    fn noon(date: NaiveDate) -> UnixTimeMinutes {
        let seconds = date.and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp();
        UnixTimeMinutes(u32::try_from(seconds / 60).unwrap())
    }

    #[test]
    fn timetable_year_changes_on_the_second_sunday_of_december() {
        assert_eq!(timetable_year(day(2026, 12, 12)), 2026);
        assert_eq!(timetable_year(day(2026, 12, 13)), 2027);
        assert_eq!(timetable_year(day(2025, 12, 13)), 2025);
        assert_eq!(timetable_year(day(2025, 12, 14)), 2026);
        assert_eq!(timetable_year(day(2026, 1, 1)), 2026);
    }

    #[test]
    fn timetable_year_handles_a_december_that_starts_on_a_sunday() {
        // 1 December 2024 is a Sunday, so the second Sunday is the 8th.
        assert_eq!(timetable_year(day(2024, 12, 7)), 2024);
        assert_eq!(timetable_year(day(2024, 12, 8)), 2025);
    }

    #[test]
    fn train_link_names_all_parts_of_the_train() {
        let links = layout_links(
            Some(page(LayoutPageKind::Train, "DB", "ICE")),
            noon(day(2026, 10, 20)),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LayoutLinkKind::Train);
        assert_eq!(
            links[0].url,
            "https://www.vagonweb.cz/razeni/vlak.php?zeme=DB&kategorie=ICE&cislo=71&rok=2026&lang=en"
        );
    }

    #[test]
    fn operator_codes_with_accents_are_percent_encoded() {
        let links = layout_links(
            Some(page(LayoutPageKind::Train, "ÖBB", "RJX")),
            noon(day(2026, 10, 20)),
        );
        assert!(links[0].url.contains("zeme=%C3%96BB&"), "{}", links[0].url);
    }

    #[test]
    fn list_links_leave_out_the_train_parts() {
        let category = layout_links(
            Some(page(LayoutPageKind::CategoryList, "ČD", "R")),
            noon(day(2026, 10, 20)),
        );
        assert_eq!(
            category[0].url,
            "https://www.vagonweb.cz/razeni/razeni.php?zeme=%C4%8CD&kategorie=R&rok=2026&lang=en"
        );
        let operator = layout_links(
            Some(page(LayoutPageKind::OperatorList, "DB", "")),
            noon(day(2026, 10, 20)),
        );
        assert_eq!(operator[0].kind, LayoutLinkKind::OperatorList);
        assert_eq!(
            operator[0].url,
            "https://www.vagonweb.cz/razeni/razeni.php?zeme=DB&rok=2026&lang=en"
        );
    }

    #[test]
    fn no_link_for_a_ride_in_another_timetable_year() {
        let train = || Some(page(LayoutPageKind::Train, "DB", "ICE"));
        // The page is for the timetable year 2026, which runs from 14 December 2025 to 12 December 2026.
        assert_eq!(layout_links(train(), noon(day(2025, 12, 13))).len(), 0);
        assert_eq!(layout_links(train(), noon(day(2025, 12, 14))).len(), 1);
        assert_eq!(layout_links(train(), noon(day(2026, 12, 12))).len(), 1);
        assert_eq!(layout_links(train(), noon(day(2026, 12, 13))).len(), 0);
    }

    #[test]
    fn no_link_without_a_page() {
        assert_eq!(layout_links(None, noon(day(2026, 10, 20))).len(), 0);
    }
}
