//! Finds dealer sites from live web search; no curated list or invented results.
//!
//! The public HTML providers can change or refuse automated access. In that case
//! callers receive actionable warnings. An optional Brave API key uses the
//! documented endpoint instead. Neither search snippets nor search prices are
//! treated as verified listings: the collector must inspect each dealer page.

use crate::model::{Garage, Search, Selectors};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::{Client, Response};
use scraper::{Html, Selector};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::thread;
use std::time::{Duration, Instant};
use url::{Host, Url};

const MAX_BODY: u64 = 3 * 1024 * 1024;
const MAX_SOURCES: usize = 36;
const MAX_PER_HOST: usize = 3;
const DISCOVERY_BUDGET: Duration = Duration::from_secs(85);
const PORTALS: &[&str] = &[
    "leboncoin.fr",
    "lacentrale.fr",
    "autoscout24.fr",
    "autoscout24.com",
    "autoscout24.be",
    "paruvendu.fr",
    "largus.fr",
    "autoplus.fr",
    "auto-moto.com",
    "auto-selection.com",
    "annonces-automobile.com",
    "vivastreet.com",
    "ouestfrance-auto.com",
    "automobile.fr",
    "mobile.de",
    "autosphere.fr",
    "caradisiac.com",
    "autohero.com",
    "aramisauto.com",
    "spoticar.fr",
    "zoomcar.fr",
    "autovisual.com",
    "autouncle.fr",
    "leparking.fr",
    "theparking.eu",
    "facebook.com",
    "instagram.com",
    "youtube.com",
    "tiktok.com",
    "x.com",
    "twitter.com",
    "pinterest.com",
    "linkedin.com",
    "wikipedia.org",
    "reddit.com",
    "amazon.fr",
    "ebay.fr",
    "google.com",
    "google.fr",
    "bing.com",
    "duckduckgo.com",
    "brave.com",
    "hackerone.com",
];

pub struct DiscoveryResult {
    pub garages: Vec<Garage>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
struct Candidate {
    url: String,
    text: String,
}

/// Search several formulations, retain diverse dealer URLs, and leave final
/// vehicle/model/budget verification to the collector and matching engine.
pub fn discover(search: &Search, brave_api_key: Option<&str>) -> Result<DiscoveryResult> {
    let queries = search_queries(search)?;
    let client = Client::builder()
        .user_agent("GarageDistiller/0.1 (+https://github.com/francoispeyret/garage-distiller)")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .context("Impossible d'initialiser la recherche web")?;
    let started = Instant::now();
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();
    let mut successful_queries = 0;

    if let Some(key) = brave_api_key.map(str::trim).filter(|key| !key.is_empty()) {
        // A separate client prevents a redirect from forwarding the secret.
        let api_client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        for (index, query) in queries.iter().enumerate() {
            if started.elapsed() >= DISCOVERY_BUDGET {
                break;
            }
            if index > 0 {
                thread::sleep(Duration::from_millis(1800));
            }
            match search_brave_api(&api_client, query, key) {
                Ok(found) => {
                    candidates.extend(found);
                    successful_queries += 1;
                }
                Err(error) => {
                    warnings.push(format!(
                        "API Brave : {error}. Essai de la recherche publique."
                    ));
                    break; // Do not repeat failed authentication or a rate limit.
                }
            }
        }
    }

    if select_garages(&candidates, search).is_empty() {
        for (index, query) in queries.iter().enumerate() {
            if started.elapsed() >= DISCOVERY_BUDGET {
                break;
            }
            if index > 0 {
                thread::sleep(Duration::from_millis(1800));
            }
            match search_brave_public(&client, query) {
                Ok(found) => {
                    candidates.extend(found);
                    successful_queries += 1;
                }
                Err(error) => {
                    warnings.push(format!("Recherche publique Brave : {error}"));
                    break; // Respect blocks; do not try to bypass challenges.
                }
            }
        }
    }

    let mut garages = select_garages(&candidates, search);
    if garages.is_empty() && started.elapsed() < DISCOVERY_BUDGET {
        for query in queries.iter().take(2) {
            if started.elapsed() >= DISCOVERY_BUDGET {
                break;
            }
            match search_duckduckgo(&client, query) {
                Ok(found) => {
                    candidates.extend(found);
                    successful_queries += 1;
                }
                Err(error) => {
                    warnings.push(format!("Recherche publique DuckDuckGo : {error}"));
                    break;
                }
            }
            thread::sleep(Duration::from_millis(1800));
        }
        garages = select_garages(&candidates, search);
    }

    if started.elapsed() >= DISCOVERY_BUDGET {
        warnings.push(
            "Recherche web interrompue à sa limite de temps ; les résultats sont partiels.".into(),
        );
    }
    if garages.is_empty() {
        if successful_queries == 0 {
            bail!(
                "Recherche web indisponible. {} Configurez GARAGE_DISTILLER_BRAVE_API_KEY pour utiliser l'API officielle, puis réessayez.",
                warnings.join(" · ")
            );
        }
        warnings.push("Aucun site de garage pertinent trouvé pour ce modèle. Les résultats web ne couvrent pas tous les garages ; essayez une désignation plus courte.".into());
    }
    warnings.sort();
    warnings.dedup();
    Ok(DiscoveryResult { garages, warnings })
}

fn search_queries(search: &Search) -> Result<Vec<String>> {
    // Quote only user words, never permit user text to inject search operators.
    let clean = |value: &str| {
        value
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let make = clean(&search.make);
    let model = clean(&search.model);
    if make.is_empty() || model.is_empty() {
        bail!("Indiquez une marque et un modèle avant la recherche web.");
    }
    if make.chars().count() > 60 || model.chars().count() > 90 {
        bail!("La marque ou le modèle est trop long pour la recherche web.");
    }
    let energy = if search.filters.electric_only {
        " électrique"
    } else {
        ""
    };
    let base = format!("\"{make}\" \"{model}\" occasion{energy}");
    let exclude = "-site:leboncoin.fr -site:lacentrale.fr -site:autoscout24.fr -site:paruvendu.fr -site:largus.fr";
    Ok(vec![
        format!("{base} garage France {exclude}"),
        format!("{base} véhicules disponibles concession {exclude}"),
        format!("{base} stock automobile vendeur professionnel {exclude}"),
        format!(
            "{base} garage moins de {} euros {exclude}",
            search.max_price
        ),
    ])
}

fn search_brave_api(client: &Client, query: &str, key: &str) -> Result<Vec<Candidate>> {
    let mut token =
        reqwest::header::HeaderValue::from_str(key).map_err(|_| anyhow!("clé API invalide"))?;
    token.set_sensitive(true);
    let response = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", token)
        .header(reqwest::header::ACCEPT, "application/json")
        .query(&[
            ("q", query),
            ("country", "FR"),
            ("search_lang", "fr"),
            ("count", "20"),
            ("result_filter", "web"),
            ("safesearch", "moderate"),
        ])
        .send()
        .map_err(|_| anyhow!("connexion impossible ou délai dépassé"))?;
    let body = read_response(response)?;
    parse_brave_api(&body)
}

fn parse_brave_api(body: &str) -> Result<Vec<Candidate>> {
    let json: Value = serde_json::from_str(body).context("réponse API illisible")?;
    if json.get("error").is_some() {
        bail!("le service de recherche a refusé la requête");
    }
    let Some(results) = json.pointer("/web/results").and_then(Value::as_array) else {
        // The API can legitimately omit `web` on an empty search.
        if json.get("query").is_some() {
            return Ok(Vec::new());
        }
        bail!("format de réponse API inconnu");
    };
    Ok(results
        .iter()
        .filter_map(|entry| {
            Some(Candidate {
                url: entry.get("url")?.as_str()?.to_owned(),
                text: format!(
                    "{} {}",
                    entry.get("title").and_then(Value::as_str).unwrap_or(""),
                    entry
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                ),
            })
        })
        .collect())
}

fn search_brave_public(client: &Client, query: &str) -> Result<Vec<Candidate>> {
    let response = client
        .get("https://search.brave.com/search")
        .query(&[("q", query), ("source", "web"), ("country", "fr")])
        .header(reqwest::header::ACCEPT_LANGUAGE, "fr-FR,fr;q=0.9")
        .send()
        .map_err(|_| anyhow!("connexion impossible ou délai dépassé"))?;
    parse_brave_html(&read_response(response)?)
}

fn parse_brave_html(body: &str) -> Result<Vec<Candidate>> {
    let html = Html::parse_document(body);
    let snippets =
        Selector::parse(".snippet[data-type='web'], #web .snippet:not([data-type])").unwrap();
    let links = Selector::parse("a[href]").unwrap();
    let mut candidates = Vec::new();
    for snippet in html.select(&snippets) {
        let context = snippet.text().collect::<Vec<_>>().join(" ");
        for link in snippet.select(&links) {
            if let Some(href) = link.value().attr("href")
                && (href.starts_with("https://") || href.starts_with("http://"))
            {
                candidates.push(Candidate {
                    url: href.into(),
                    text: context.clone(),
                });
            }
        }
    }
    if candidates.is_empty() {
        let lower = body.to_ascii_lowercase();
        if lower.contains("no results found") || lower.contains("aucun résultat") {
            return Ok(Vec::new());
        }
        bail!("aucun résultat lisible (protection anti-robot ou format du moteur modifié)");
    }
    Ok(candidates)
}

fn search_duckduckgo(client: &Client, query: &str) -> Result<Vec<Candidate>> {
    let response = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query), ("kl", "fr-fr")])
        .send()
        .map_err(|_| anyhow!("connexion impossible ou délai dépassé"))?;
    parse_duckduckgo_html(&read_response(response)?)
}

fn parse_duckduckgo_html(body: &str) -> Result<Vec<Candidate>> {
    if body.contains("anomaly-modal") || body.contains("challenge-form") {
        bail!("vérification anti-robot demandée par le moteur");
    }
    let html = Html::parse_document(body);
    let results = Selector::parse(".result").unwrap();
    let links = Selector::parse("a.result__a").unwrap();
    let mut candidates = Vec::new();
    for result in html.select(&results) {
        for link in result.select(&links) {
            if let Some(href) = link.value().attr("href") {
                candidates.push(Candidate {
                    url: href.into(),
                    text: result.text().collect::<Vec<_>>().join(" "),
                });
            }
        }
    }
    if candidates.is_empty() && !body.contains("no-results") && !body.contains("No results found") {
        bail!("format de résultats non reconnu");
    }
    Ok(candidates)
}

fn read_response(response: Response) -> Result<String> {
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => anyhow!("accès refusé (clé invalide ou protection anti-robot)"),
            429 => anyhow!("limite de requêtes atteinte ; réessayez plus tard"),
            _ => anyhow!("service indisponible (HTTP {})", status.as_u16()),
        });
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BODY)
    {
        bail!("réponse trop volumineuse");
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .context("lecture de la réponse interrompue")?;
    if bytes.len() as u64 > MAX_BODY {
        bail!("réponse trop volumineuse");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn select_garages(candidates: &[Candidate], search: &Search) -> Vec<Garage> {
    let mut seen = HashSet::new();
    let mut per_host: HashMap<String, usize> = HashMap::new();
    let mut chosen: Vec<(usize, String, Url)> = Vec::new();
    for candidate in candidates {
        let Some(url) = clean_candidate_url(&candidate.url) else {
            continue;
        };
        if !relevant(candidate, &url, search) || !seen.insert(url.to_string()) {
            continue;
        }
        let host = url
            .host_str()
            .unwrap_or_default()
            .trim_start_matches("www.")
            .to_owned();
        let count = per_host.entry(host.clone()).or_default();
        if *count >= MAX_PER_HOST {
            continue;
        }
        chosen.push((*count, host, url));
        *count += 1;
    }
    // Interleave hosts so a large dealer never monopolizes the source budget.
    chosen.sort_by_key(|(rank, _, _)| *rank);
    chosen
        .into_iter()
        .take(MAX_SOURCES)
        .map(|(_, host, url)| {
            let address = url.to_string();
            let id = format!("web-{:x}", Sha256::digest(address.as_bytes()));
            Garage {
                id,
                name: host,
                url: address,
                enabled: true,
                selectors: Selectors::default(),
                last_scan: None,
                last_error: None,
            }
        })
        .collect()
}

fn relevant(candidate: &Candidate, url: &Url, search: &Search) -> bool {
    let haystack = normalize(&format!("{} {}", candidate.text, url));
    let words: HashSet<&str> = haystack.split_whitespace().collect();
    let model = normalize(&search.model);
    let make = normalize(&search.make);
    // Exact word matching avoids e.g. BMW i3 results matching i30. Both make and
    // model must occur in the result, not merely the original query.
    !model.is_empty()
        && !make.is_empty()
        && model.split_whitespace().all(|word| words.contains(word))
        && make.split_whitespace().all(|word| words.contains(word))
}

fn normalize(value: &str) -> String {
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
        .collect()
}

fn clean_candidate_url(raw: &str) -> Option<Url> {
    let absolute = if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.to_owned()
    };
    let mut url = Url::parse(&absolute).ok()?;
    if url
        .host_str()
        .is_some_and(|host| host == "duckduckgo.com" || host.ends_with(".duckduckgo.com"))
    {
        let target = url
            .query_pairs()
            .find(|(key, _)| key == "uddg")?
            .1
            .into_owned();
        url = Url::parse(&target).ok()?;
    }
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let host = match url.host()? {
        Host::Domain(host) => host.to_ascii_lowercase(),
        _ => return None,
    };
    if !host.contains('.')
        || host.ends_with(".local")
        || host.ends_with(".localhost")
        || host.ends_with(".internal")
        || host.ends_with(".test")
        || host.ends_with(".example")
        || PORTALS
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return None;
    }
    let path = url.path().to_ascii_lowercase();
    if [".pdf", ".jpg", ".png", ".zip", ".mp4"]
        .iter()
        .any(|ext| path.ends_with(ext))
    {
        return None;
    }
    url.set_fragment(None);
    let retained: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| {
            !key.starts_with("utm_") && !matches!(key.as_ref(), "gclid" | "fbclid" | "msclkid")
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !retained.is_empty() {
        url.query_pairs_mut().extend_pairs(retained);
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search() -> Search {
        Search {
            id: "search-1".into(),
            make: "Renault".into(),
            model: "Clio".into(),
            max_price: 12000,
            filters: Default::default(),
            enabled: true,
            created_at: 0,
        }
    }

    #[test]
    fn brave_fixture_keeps_dealer_and_excludes_marketplace() {
        let candidates =
            parse_brave_html(include_str!("../tests/fixtures/discovery/brave.html")).unwrap();
        let garages = select_garages(&candidates, &search());
        assert_eq!(garages.len(), 2);
        assert_eq!(garages[0].name, "garage-a.fr");
        assert_eq!(garages[0].url, "https://garage-a.fr/occasion/renault-clio");
        assert!(garages.iter().all(|g| g.selectors.card.is_empty()));
    }

    #[test]
    fn unwraps_ddg_and_preserves_inventory_query() {
        let candidates =
            parse_duckduckgo_html(include_str!("../tests/fixtures/discovery/duckduckgo.html"))
                .unwrap();
        let garages = select_garages(&candidates, &search());
        assert_eq!(garages.len(), 1);
        assert_eq!(garages[0].url, "https://garage-a.fr/stock?model=clio");
    }

    #[test]
    fn does_not_treat_a_captcha_or_unknown_html_as_empty_success() {
        assert!(parse_duckduckgo_html("<form id='challenge-form'>Captcha</form>").is_err());
        assert!(parse_brave_html("<html>Network verification</html>").is_err());
        assert!(
            parse_brave_html("<html>No results found</html>")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_portals_credentials_non_web_and_local_urls() {
        for url in [
            "https://pros.lacentrale.fr/stock",
            "https://foo.leboncoin.fr/a",
            "https://occasion.autoplus.fr/renault/clio",
            "https://auto-selection.com/voiture-occasion",
            "https://occasion.auto-moto.com/auto/renault/clio",
            "file:///tmp/a",
            "javascript:alert(1)",
            "https://user:secret@garage.fr/ad",
            "http://127.0.0.1/ad",
            "http://[::1]/ad",
            "http://garage.local/ad",
        ] {
            assert!(clean_candidate_url(url).is_none(), "accepted {url}");
        }
        assert!(clean_candidate_url("https://lacentrale.fr.garage-real.fr/clio").is_some());
    }

    #[test]
    fn api_results_are_candidates_not_trusted_advertisements() {
        let parsed = parse_brave_api(r#"{"web":{"results":[{"url":"https://garage-a.fr/clio","title":"Renault Clio","description":"garagiste"}]}}"#).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parse_brave_api(r#"{"error":{"detail":"secret"}}"#).is_err());
        assert!(
            parse_brave_api(r#"{"query":{"original":"Renault"}}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn requires_model_and_make_and_does_not_match_substrings() {
        let mut search = search();
        search.make = "BMW".into();
        search.model = "i3".into();
        let url = Url::parse("https://garage.fr/occasion").unwrap();
        assert!(!relevant(
            &Candidate {
                url: url.to_string(),
                text: "BMW i30 occasion".into()
            },
            &url,
            &search
        ));
        assert!(relevant(
            &Candidate {
                url: url.to_string(),
                text: "BMW i3 occasion".into()
            },
            &url,
            &search
        ));
    }

    #[test]
    fn distributes_source_budget_and_deduplicates_tracking_urls() {
        let mut candidates = Vec::new();
        for index in 0..10 {
            candidates.push(Candidate {
                url: format!("https://garage-a.fr/clio/{index}"),
                text: "Renault Clio".into(),
            });
        }
        candidates.push(Candidate {
            url: "https://garage-b.fr/clio".into(),
            text: "Renault Clio".into(),
        });
        candidates.push(Candidate {
            url: "https://garage-b.fr/clio?utm_source=search".into(),
            text: "Renault Clio".into(),
        });
        let garages = select_garages(&candidates, &search());
        assert_eq!(garages.len(), 4);
        assert_eq!(garages[1].name, "garage-b.fr");
        assert_eq!(garages[0].id, select_garages(&candidates, &search())[0].id);
    }

    #[test]
    fn query_input_cannot_add_search_operators() {
        let mut search = search();
        search.model = "Clio\" site:evil.fr".into();
        let queries = search_queries(&search).unwrap();
        assert!(!queries[0].contains("site:evil.fr"));
        search.make.clear();
        assert!(search_queries(&search).is_err());
    }
}
