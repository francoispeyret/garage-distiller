use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Search {
    pub id: String,
    pub make: String,
    pub model: String,
    pub max_price: u32,
    pub enabled: bool,
    pub created_at: i64,
}

impl Search {
    pub fn matches(&self, listing: &Listing) -> bool {
        if !self.enabled || listing.price == 0 || listing.price > self.max_price {
            return false;
        }
        let title = normalize(&listing.title);
        let contains = |needle: &str| {
            normalize(needle)
                .split_whitespace()
                .all(|word| title.split_whitespace().any(|token| token == word))
        };
        contains(&self.make) && contains(&self.model)
    }

    pub fn label(&self) -> String {
        format!("{} {}", self.make.trim(), self.model.trim())
    }
}

pub fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' | 'ä' => 'a',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Selectors {
    pub card: String,
    pub title: String,
    pub price: String,
    pub link: String,
    pub mileage: String,
    pub year: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Garage {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    #[serde(default)]
    pub selectors: Selectors,
    pub last_scan: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Listing {
    pub id: String,
    pub garage_id: String,
    pub garage_name: String,
    pub title: String,
    pub price: u32,
    pub url: String,
    pub mileage: Option<u32>,
    pub year: Option<u16>,
    pub fuel: Option<String>,
    pub first_seen: i64,
    pub last_seen: i64,
    #[serde(default)]
    pub favorite: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub listing_id: String,
    pub search_id: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    pub read: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub scan_minutes: u32,
    pub auto_scan: bool,
    pub notifications: bool,
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scan_minutes: 60,
            auto_scan: true,
            notifications: true,
            auto_update: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub schema_version: u32,
    pub searches: Vec<Search>,
    pub garages: Vec<Garage>,
    pub listings: Vec<Listing>,
    pub alerts: Vec<Alert>,
    pub notified: BTreeSet<String>,
    pub settings: Settings,
    pub last_scan: Option<i64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema_version: 1,
            searches: vec![],
            garages: vec![],
            listings: vec![],
            alerts: vec![],
            notified: BTreeSet::new(),
            settings: Settings::default(),
            last_scan: None,
        }
    }
}

impl State {
    /// Keep user state and original discovery date when a price changes.
    pub fn merge_listings(&mut self, incoming: Vec<Listing>) -> Vec<Alert> {
        let mut alerts = Vec::new();
        for mut listing in incoming {
            if let Some(old) = self.listings.iter().find(|old| old.id == listing.id) {
                listing.favorite = old.favorite;
                listing.first_seen = old.first_seen;
            }
            for search in &self.searches {
                if search.matches(&listing) {
                    let key = format!("{}:{}:{}", search.id, listing.id, listing.price);
                    if self.notified.insert(key.clone()) {
                        alerts.push(Alert {
                            id: key,
                            listing_id: listing.id.clone(),
                            search_id: search.id.clone(),
                            title: listing.title.clone(),
                            body: format!(
                                "{} € chez {} · budget {} €",
                                listing.price, listing.garage_name, search.max_price
                            ),
                            created_at: chrono::Utc::now().timestamp(),
                            read: false,
                        });
                    }
                }
            }
            if let Some(old) = self.listings.iter_mut().find(|old| old.id == listing.id) {
                *old = listing;
            } else {
                self.listings.push(listing);
            }
        }
        self.alerts.extend(alerts.iter().cloned());
        alerts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn listing() -> Listing {
        Listing {
            id: "car-1".into(),
            garage_id: "garage".into(),
            garage_name: "Garage".into(),
            title: "Peugeot 208 PureTech".into(),
            price: 12000,
            url: "https://garage.example/208".into(),
            mileage: Some(42000),
            year: Some(2021),
            fuel: None,
            first_seen: 10,
            last_seen: 20,
            favorite: false,
        }
    }
    fn search() -> Search {
        Search {
            id: "s1".into(),
            make: "Peugeot".into(),
            model: "208".into(),
            max_price: 12000,
            enabled: true,
            created_at: 1,
        }
    }
    #[test]
    fn model_and_price_are_exact_constraints() {
        let mut car = listing();
        let s = search();
        assert!(s.matches(&car));
        car.price = 12001;
        assert!(!s.matches(&car));
        car.price = 11000;
        car.title = "Peugeot 2008".into();
        assert!(!s.matches(&car));
        car.title = "PEUGEOT 208 - occasion".into();
        assert!(s.matches(&car));
        assert!(
            !Search {
                enabled: false,
                ..s
            }
            .matches(&car)
        );
    }
    #[test]
    fn deduplicates_and_alerts_on_new_price_without_losing_favorites() {
        let mut state = State {
            searches: vec![search()],
            ..State::default()
        };
        assert_eq!(state.merge_listings(vec![listing()]).len(), 1);
        state.listings[0].favorite = true;
        assert!(state.merge_listings(vec![listing()]).is_empty());
        let mut car = listing();
        car.price = 11000;
        car.first_seen = 30;
        assert_eq!(state.merge_listings(vec![car]).len(), 1);
        assert_eq!(state.listings.len(), 1);
        assert!(state.listings[0].favorite);
        assert_eq!(state.listings[0].first_seen, 10);
        let restored: State =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(restored.notified.len(), 2);
    }
}
