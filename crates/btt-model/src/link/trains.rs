//! The dictionary of trains that links refer to by number.
//!
//! The dictionary is the file `data/trains.tsv`, one train per line as its operator, category and
//! number, separated by tabs. The web viewer reads the same file. A link names a train by its
//! line, so lines are only ever appended, and a train that the dictionary lacks travels as a
//! literal. The example `train_dictionary` of `btt-planner` writes the file from a timetable
//! bundle. It lists the trains that cover long distances first, and a link spends the fewest bits
//! on the first entries. Only the first 16,383 lines can be named in two groups of seven bits.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::plan::TrainLabel;

/// The dictionary file, compiled in.
const DICTIONARY_FILE: &str = include_str!("../../data/trains.tsv");

/// A line of the dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DictionaryTrain {
    /// The operator as the app names it, such as `db-fernverkehr`.
    pub operator: &'static str,
    /// Product or category, such as "ICE".
    pub category: &'static str,
    /// Train number within the category.
    pub number: &'static str,
}

/// The trains of the dictionary file, and a lookup by label.
#[derive(Debug)]
pub(super) struct TrainDictionary {
    entries: Vec<DictionaryTrain>,
    positions_by_number: HashMap<&'static str, Vec<usize>>,
}

/// Returns the dictionary, which is parsed on first use.
pub(super) fn dictionary() -> &'static TrainDictionary {
    static DICTIONARY: OnceLock<TrainDictionary> = OnceLock::new();
    DICTIONARY.get_or_init(|| TrainDictionary::parse(DICTIONARY_FILE))
}

impl TrainDictionary {
    /// Reads the lines of a dictionary file. A line that is not a train is skipped, which would
    /// shift the numbers of the trains after it, so a test checks that the file has none.
    fn parse(text: &'static str) -> Self {
        let mut entries = Vec::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if let [operator, category, number] = fields[..] {
                entries.push(DictionaryTrain {
                    operator,
                    category,
                    number,
                });
            }
        }
        let mut positions_by_number: HashMap<&'static str, Vec<usize>> = HashMap::new();
        for (position, entry) in entries.iter().enumerate() {
            positions_by_number
                .entry(entry.number)
                .or_default()
                .push(position);
        }
        Self {
            entries,
            positions_by_number,
        }
    }

    /// Returns the train at a position.
    pub(super) fn get(&self, index: usize) -> Option<&DictionaryTrain> {
        self.entries.get(index)
    }

    /// Returns the number of trains.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns the position of the train that a label stands for. A train that the file lists
    /// twice keeps its first line, which is the shorter number.
    pub(super) fn find(&self, label: &TrainLabel) -> Option<usize> {
        self.positions_by_number
            .get(label.number.as_str())?
            .iter()
            .copied()
            .find(|&index| {
                let entry = &self.entries[index];
                entry.operator == label.operator && entry.category == label.category
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_of_the_file_is_a_train() {
        let lines = DICTIONARY_FILE.lines().count();
        assert_eq!(dictionary().len(), lines);
        assert!(lines >= 1000);
    }

    #[test]
    fn a_train_of_the_dictionary_finds_itself() {
        let entry = dictionary().get(0).unwrap();
        let label = TrainLabel {
            category: entry.category.into(),
            number: entry.number.into(),
            operator: entry.operator.into(),
            agency: "".into(),
        };
        assert_eq!(dictionary().find(&label), Some(0));
    }

    #[test]
    fn a_train_with_another_operator_is_not_found() {
        let entry = dictionary().get(0).unwrap();
        let label = TrainLabel {
            category: entry.category.into(),
            number: entry.number.into(),
            operator: "no-such-operator".into(),
            agency: "".into(),
        };
        assert_eq!(dictionary().find(&label), None);
    }
}
