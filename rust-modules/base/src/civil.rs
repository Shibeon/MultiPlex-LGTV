//! **The proleptic Gregorian calendar's day count**: Howard Hinnant's `days_from_civil` and its
//! inverse, pure arithmetic with no zone and no `time_t`. The one copy every layer reads (the
//! Friends feed's dates, the Library's date probes, the sort captions' "Yesterday").

/// Days from 1970-01-01 to the civil date `[year, month, day]` (month 1-12): negative before 1970.
pub fn days_from_civil([year, month, day]: [i32; 3]) -> i64 {
    let (m, d) = (i64::from(month), i64::from(day));
    let y = i64::from(year) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date `[year, month, day]` of a day count since 1970-01-01: [`days_from_civil`]'s
/// inverse.
pub fn civil_from_days(days: i64) -> [i32; 3] {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    [y as i32, m as i32, d as i32]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates_and_the_round_trip() {
        assert_eq!(days_from_civil([1970, 1, 1]), 0);
        assert_eq!(days_from_civil([1969, 12, 31]), -1);
        assert_eq!(days_from_civil([2000, 3, 1]), 11_017);
        assert_eq!(days_from_civil([2026, 11, 15]), 20_772);
        assert_eq!(days_from_civil([1600, 2, 29]), -135_081);
        for days in (-800_000..800_000).step_by(997) {
            assert_eq!(days_from_civil(civil_from_days(days)), days);
        }
        assert_eq!(civil_from_days(20_772), [2026, 11, 15]);
    }
}
