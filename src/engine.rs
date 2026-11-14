use crate::{
    collector, discovery,
    model::{Garage, Listing, Search},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::Sender,
};

#[derive(Debug)]
pub enum Event {
    Progress(String),
    Discovered(Vec<Garage>),
    Site {
        garage: Box<Garage>,
        listings: Vec<Listing>,
        done: usize,
        total: usize,
    },
    Warning(String),
    Finished {
        cancelled: bool,
    },
}

/// All network work happens off the UI thread, with at most three sites in flight.
pub fn run(
    searches: Vec<Search>,
    known: Vec<Garage>,
    api_key: Option<String>,
    tx: Sender<Event>,
    cancel: Arc<AtomicBool>,
) {
    let mut garages: Vec<Garage> = known.iter().filter(|g| g.enabled).cloned().collect();
    let disabled: Vec<_> = known
        .iter()
        .filter(|g| !g.enabled)
        .map(|g| &g.url)
        .collect();
    for search in searches.iter().filter(|s| s.enabled) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let _ = tx.send(Event::Progress(format!(
            "Recherche sur le web · {}",
            search.label()
        )));
        match discovery::discover(search, api_key.as_deref()) {
            Ok(result) => {
                for warning in result.warnings {
                    let _ = tx.send(Event::Warning(warning));
                }
                for garage in result.garages {
                    if !disabled.contains(&&garage.url)
                        && !garages.iter().any(|g| g.url == garage.url)
                    {
                        garages.push(garage);
                    }
                }
            }
            Err(error) => {
                let _ = tx.send(Event::Warning(format!(
                    "Découverte {} : {error:#}",
                    search.label()
                )));
            }
        }
    }
    // Revisit recent and newly discovered candidates; keep the whole discovered set in the UI.
    let _ = tx.send(Event::Discovered(garages.clone()));
    // Bound a pass and rotate candidates across passes so every saved source is eventually revisited.
    garages.sort_by_key(|g| g.last_scan.unwrap_or(0));
    garages.truncate(60);
    let total = garages.len();
    // Each host is owned by one worker, so its pages never bypass courtesy delays
    // through parallel fetchers. Other hosts can still make progress independently.
    let mut hosts = std::collections::BTreeMap::<String, Vec<Garage>>::new();
    for garage in garages {
        let host = url::Url::parse(&garage.url)
            .ok()
            .and_then(|u| {
                u.host_str()
                    .map(|h| h.trim_start_matches("www.").to_owned())
            })
            .unwrap_or_else(|| garage.id.clone());
        hosts.entry(host).or_default().push(garage);
    }
    let groups: Vec<_> = hosts.into_values().collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..3 {
            scope.spawn(|| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(group) = groups.get(index) else {
                        break;
                    };
                    for garage in group {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let mut garage = garage.clone();
                        let _ = tx.send(Event::Progress(format!(
                            "Lecture du site · {}",
                            garage.name
                        )));
                        let result = collector::scan_garage(&garage);
                        garage.last_scan = Some(chrono::Utc::now().timestamp());
                        let listings = match result {
                            Ok(listings) => {
                                garage.last_error = None;
                                listings
                            }
                            Err(error) => {
                                garage.last_error = Some(format!("{error:#}"));
                                Vec::new()
                            }
                        };
                        let completed = done.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = tx.send(Event::Site {
                            garage: Box::new(garage),
                            listings,
                            done: completed,
                            total,
                        });
                    }
                }
            });
        }
    });
    let _ = tx.send(Event::Finished {
        cancelled: cancel.load(Ordering::Relaxed),
    });
}
