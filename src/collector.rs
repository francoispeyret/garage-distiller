//! Collection des annonces publiées directement par un garage.
//!
//! Aucun moteur JavaScript ni contournement anti-bot : JSON-LD, microdonnées et
//! cartes HTML sont lus avec des limites explicites et le respect de robots.txt.

use crate::model::{Garage, Listing, Selectors};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::{Client, Response};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const AGENT: &str = "GarageDistiller";
const USER_AGENT: &str =
    "GarageDistiller/0.1 (+https://github.com/francoispeyret/garage-distiller)";
const MAX_BODY: u64 = 3 * 1024 * 1024;
const MAX_DETAIL_PAGES: usize = 10;
const SCAN_BUDGET: Duration = Duration::from_secs(60);
const TITLE_SELECTORS: &str = "[itemprop='name'], h2, h3, h4, .vehicle-title, .car-title, .title, [class*='vehicle-name'], [class*='voiture-titre']";
const PRICE_SELECTORS: &str =
    "[itemprop='price'], [data-price], .price, .prix, [class*='price'], [class*='prix']";

/// Scan one directly configured or automatically discovered garage page.
/// Empty/JavaScript-only/blocked pages produce an actionable diagnostic.
pub fn scan_garage(garage: &Garage) -> Result<Vec<Listing>> {
    let start = validate_url(&garage.url)?;
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(18))
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut fetcher = Fetcher {
        client,
        robots: HashMap::new(),
        last_request: None,
        started: Instant::now(),
    };
    let (base, html) = fetcher.page(start.clone(), &start)?;
    let mut listings = extract_listings(&html, &base, garage)?;
    // The initial page may be an inventory with structured URLs rather than
    // prices. Follow a small, deterministic selection of vehicle details.
    if listings.is_empty() && garage.selectors.card.trim().is_empty() {
        let links = detail_links(&html, &base);
        let mut failures = Vec::new();
        for link in links.into_iter().take(MAX_DETAIL_PAGES) {
            if fetcher.started.elapsed() >= SCAN_BUDGET {
                break;
            }
            match fetcher.page(link, &base) {
                Ok((final_url, body)) => {
                    listings.extend(extract_listings(&body, &final_url, garage)?)
                }
                Err(error) => failures.push(error.to_string()),
            }
        }
        if listings.is_empty() && !failures.is_empty() {
            bail!("Aucune annonce exploitable. {}", failures[0]);
        }
    }
    let mut unique = BTreeMap::new();
    for listing in listings {
        unique.insert(listing.id.clone(), listing);
    }
    if unique.is_empty() {
        bail!(
            "Aucune annonce avec un prix d'achat en euros n'a pu être lue. Le site peut être vide, chargé en JavaScript ou nécessiter des sélecteurs CSS adaptés."
        );
    }
    Ok(unique.into_values().collect())
}

struct Robots {
    text: String,
    delay: Duration,
}

struct Fetcher {
    client: Client,
    robots: HashMap<String, Robots>,
    last_request: Option<Instant>,
    started: Instant,
}

impl Fetcher {
    fn remaining(&self) -> Result<Duration> {
        let remaining = SCAN_BUDGET.saturating_sub(self.started.elapsed());
        if remaining.is_zero() {
            bail!("Limite de 60 secondes atteinte pour ce garage ; réessayez plus tard");
        }
        Ok(remaining.min(Duration::from_secs(18)))
    }

    fn robots(&mut self, url: &Url) -> Result<()> {
        let origin = url.origin().ascii_serialization();
        if !self.robots.contains_key(&origin) {
            let mut robots_url = url.join("/robots.txt")?;
            let mut body = None;
            for _ in 0..6 {
                let response = self
                    .client
                    .get(robots_url.clone())
                    .timeout(self.remaining()?)
                    .send()
                    .context("Impossible de vérifier robots.txt")?;
                self.last_request = Some(Instant::now());
                if response.status().is_redirection() {
                    robots_url = redirect_target(&response, &robots_url, url)?;
                    continue;
                }
                body = Some(if matches!(response.status().as_u16(), 404 | 410) {
                    String::new()
                } else {
                    let response = response
                        .error_for_status()
                        .context("Lecture de robots.txt refusée ; collecte suspendue")?;
                    bounded_text(response, 512 * 1024)?
                });
                break;
            }
            let text = body.ok_or_else(|| anyhow!("Trop de redirections de robots.txt"))?;
            // A conservative maximum also respects non-standard Crawl-delay.
            let delay = crawl_delay(&text);
            if delay > Duration::from_secs(30) {
                bail!(
                    "Le délai robots.txt dépasse 30 secondes ; ce site nécessite un connecteur adapté."
                );
            }
            self.robots.insert(origin.clone(), Robots { text, delay });
        }
        let policy = &self.robots[&origin];
        let mut matcher = robotstxt::DefaultMatcher::default();
        if !matcher.one_agent_allowed_by_robots(&policy.text, AGENT, url.as_str()) {
            bail!(
                "La collecte de cette page est interdite par robots.txt : {}",
                url.path()
            );
        }
        if let Some(previous) = self.last_request {
            let delay = policy.delay.saturating_sub(previous.elapsed());
            if delay >= SCAN_BUDGET.saturating_sub(self.started.elapsed()) {
                bail!("Limite de temps atteinte avant la prochaine requête autorisée");
            }
            thread::sleep(delay);
        }
        Ok(())
    }

    fn page(&mut self, mut url: Url, site: &Url) -> Result<(Url, String)> {
        for _ in 0..6 {
            validate_url(url.as_str())?;
            if !same_site(&url, site) {
                bail!("Redirection vers un autre site refusée");
            }
            self.robots(&url)?;
            let response = self
                .client
                .get(url.clone())
                .timeout(self.remaining()?)
                .send()
                .with_context(|| {
                    format!("Le site {} ne répond pas", url.host_str().unwrap_or(""))
                })?;
            self.last_request = Some(Instant::now());
            if response.status().is_redirection() {
                url = redirect_target(&response, &url, site)?;
                continue;
            }
            let response = response
                .error_for_status()
                .context("Le garage a refusé la requête")?;
            if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
                let content_type = content_type.to_str().unwrap_or("");
                if !content_type.contains("html") && !content_type.contains("text/plain") {
                    bail!("Cette adresse ne contient pas une page HTML");
                }
            }
            let html = bounded_text(response, MAX_BODY)?;
            let lower = html.to_ascii_lowercase();
            if lower.contains("<title>just a moment")
                || lower.contains("<title>access denied")
                || lower.contains("cf-chl-")
            {
                bail!("Le site demande une vérification anti-bot ; aucun contournement effectué");
            }
            return Ok((url, html));
        }
        bail!("Trop de redirections du site du garage")
    }
}

fn bounded_text(response: Response, limit: u64) -> Result<String> {
    if response.content_length().is_some_and(|size| size > limit) {
        bail!("Page trop volumineuse");
    }
    let mut bytes = Vec::new();
    response.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("Page trop volumineuse");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn redirect_target(response: &Response, current: &Url, site: &Url) -> Result<Url> {
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or_else(|| anyhow!("Redirection sans destination"))?
        .to_str()?;
    let next = current.join(location)?;
    validate_url(next.as_str())?;
    if !same_site(&next, site) || (current.scheme() == "https" && next.scheme() != "https") {
        bail!("Redirection hors du site du garage refusée");
    }
    Ok(next)
}

fn crawl_delay(body: &str) -> Duration {
    let seconds = body
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split('#').next()?.split_once(':')?;
            if key.trim().eq_ignore_ascii_case("crawl-delay") {
                value.trim().parse::<f64>().ok()
            } else {
                None
            }
        })
        .filter(|n| n.is_finite() && *n >= 0.0)
        .fold(1.0, f64::max);
    Duration::from_secs_f64(seconds.min(86_400.0))
}

/// Public so discovery and forms can reject classifieds and malformed URLs.
pub fn validate_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("Adresse du garage invalide")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Utilisez une adresse HTTP(S) sans identifiants");
    }
    let host = url.host_str().unwrap_or("").trim_start_matches("www.");
    const PORTALS: &[&str] = &[
        "leboncoin.fr",
        "lacentrale.fr",
        "autoscout24.fr",
        "autoscout24.com",
        "paruvendu.fr",
        "ouestfrance-auto.com",
        "leparking.fr",
        "autotrader.co.uk",
        "mobile.de",
        "facebook.com",
    ];
    if PORTALS
        .iter()
        .any(|portal| host == *portal || host.ends_with(&format!(".{portal}")))
    {
        bail!("Cette adresse est un portail d'annonces ; ajoutez le site direct d'un garage");
    }
    Ok(url)
}

fn same_site(a: &Url, b: &Url) -> bool {
    a.host_str().map(|s| s.trim_start_matches("www."))
        == b.host_str().map(|s| s.trim_start_matches("www."))
        && (a.port_or_known_default() == b.port_or_known_default()
            || (a.port().is_none() && b.port().is_none()))
}

fn listing_url(raw: &str, base: &Url) -> Option<Url> {
    let mut url = base.join(raw.trim()).ok()?;
    validate_url(url.as_str()).ok()?;
    if !same_site(&url, base) {
        return None;
    }
    url.set_fragment(None);
    let query: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !key.starts_with("utm_") && !matches!(key.as_ref(), "fbclid" | "gclid"))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Some(url)
}

fn selector(raw: &str) -> Result<Selector> {
    Selector::parse(raw).map_err(|error| anyhow!("Sélecteur CSS invalide « {raw} » : {error:?}"))
}

fn text(element: ElementRef<'_>) -> String {
    element
        .value()
        .attr("content")
        .or_else(|| element.value().attr("data-price"))
        .map(str::to_owned)
        .unwrap_or_else(|| element.text().collect::<Vec<_>>().join(" "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn select_text(element: ElementRef<'_>, raw: &str) -> Option<String> {
    let parsed = selector(raw).ok()?;
    if parsed.matches(&element) {
        return Some(text(element));
    }
    element.select(&parsed).next().map(text)
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Object(map) => map
            .get("name")
            .or_else(|| map.get("value"))
            .and_then(json_text),
        _ => None,
    }
}

fn financing(raw: &str) -> bool {
    let value = raw.to_lowercase();
    [
        "/mois",
        "/ mois",
        "par mois",
        "mensual",
        "loyer",
        "/month",
        "per month",
        "monthly",
        "leasing",
        "à partir",
        "a partir",
        "dès ",
        "des ",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn cash_price(raw: &str, known_eur: bool) -> Option<u32> {
    let value = raw.to_lowercase();
    if financing(&value) {
        return None;
    }
    if ["usd", "gbp", "chf", "$", "£"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        return None;
    }
    if !known_eur && !value.contains('€') && !value.contains("eur") {
        return None;
    }
    let start = value.find(|c: char| c.is_ascii_digit())?;
    let amount: String = value[start..]
        .chars()
        .take_while(|c| {
            c.is_ascii_digit() || c.is_whitespace() || matches!(c, '.' | ',' | '\'' | '’')
        })
        .collect();
    let mut compact: String = amount
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '\'' | '’'))
        .collect();
    // The final 1–2 digits denote cents; three denote a thousands group.
    if let Some(last) = compact.rfind(['.', ','])
        && compact.len() - last - 1 <= 2
    {
        let cents = &compact[last + 1..];
        if cents.chars().any(|c| c != '0') {
            return None;
        }
        compact.truncate(last);
    }
    let integer: String = compact.chars().filter(char::is_ascii_digit).collect();
    let price = integer.parse::<u32>().ok()?;
    (500..=10_000_000).contains(&price).then_some(price)
}

fn integer(raw: &str) -> Option<u32> {
    let start = raw.find(|c: char| c.is_ascii_digit())?;
    let digits: String = raw[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || c.is_whitespace() || matches!(c, '.' | ','))
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn year(raw: &str) -> Option<u16> {
    raw.split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse::<u16>().ok())
        .find(|value| (1950..=2100).contains(value))
}

fn make_listing(
    garage: &Garage,
    title: String,
    price: u32,
    url: Url,
    mileage: Option<u32>,
    year: Option<u16>,
    fuel: Option<String>,
) -> Listing {
    let now = chrono::Utc::now().timestamp();
    Listing {
        id: format!("{:x}", Sha256::digest(url.as_str().as_bytes())),
        garage_id: garage.id.clone(),
        garage_name: garage.name.clone(),
        title,
        price,
        url: url.into(),
        mileage,
        year,
        fuel,
        first_seen: now,
        last_seen: now,
        favorite: false,
    }
}

fn is_vehicle_title(title: &str) -> bool {
    let title = title.trim().to_lowercase();
    let words = crate::model::normalize(&title);
    title.len() >= 4
        && title.len() <= 240
        && !words.split_whitespace().any(|word| {
            matches!(
                word,
                "vendu" | "vendue" | "sold" | "accessoire" | "accessoires"
            )
        })
        && ![
            "pièce détachée",
            "pièces détachées",
            "pieces detachees",
            "pièce pour",
            "pièces pour",
            "moteur pour",
            "pneus pour",
            "jantes pour",
        ]
        .iter()
        .any(|phrase| title.contains(phrase))
        && ![
            "pneu ",
            "pneus ",
            "jante ",
            "jantes ",
            "pièce détachée",
            "pieces detachees",
            "tapis de sol",
            "location de voiture",
        ]
        .iter()
        .any(|phrase| title.starts_with(phrase))
}

fn unavailable(raw: &str) -> bool {
    let lower = crate::model::normalize(raw);
    lower.split_whitespace().any(|word| {
        matches!(
            word,
            "outofstock"
                | "soldout"
                | "discontinued"
                | "vendu"
                | "vendue"
                | "reserve"
                | "reservee"
                | "sold"
        )
    })
}

fn recurring_price(value: &Value) -> bool {
    if let Some(values) = value.as_array() {
        return values.iter().any(recurring_price);
    }
    value.get("billingDuration").is_some()
        || value.get("billingIncrement").is_some()
        || value
            .get("unitCode")
            .and_then(Value::as_str)
            .is_some_and(|unit| matches!(unit, "MON" | "DAY" | "HUR"))
}

fn rejected_offer(value: &Value) -> bool {
    if let Some(values) = value.as_array() {
        return !values.is_empty() && values.iter().all(rejected_offer);
    }
    value
        .get("availability")
        .and_then(json_text)
        .is_some_and(|raw| unavailable(&raw))
        || value.get("priceSpecification").is_some_and(recurring_price)
}

fn json_offer(value: &Value) -> Option<(u32, Option<String>)> {
    if let Some(offers) = value.as_array() {
        return offers.iter().find_map(json_offer);
    }
    let currency = value
        .get("priceCurrency")
        .and_then(json_text)
        .unwrap_or_default();
    if !currency.eq_ignore_ascii_case("EUR") {
        return None;
    }
    if rejected_offer(value) {
        return None;
    }
    let specification = value.get("priceSpecification");
    // AggregateOffer.lowPrice may represent a range or a financing offer.
    let price = value
        .get("price")
        .or_else(|| specification.and_then(|p| p.get("price")))
        .and_then(json_text)?;
    let price = cash_price(&price, true)?;
    Some((price, value.get("url").and_then(json_text)))
}

fn walk_json(
    value: &Value,
    base: &Url,
    garage: &Garage,
    listings: &mut Vec<Listing>,
    excluded: &mut HashSet<String>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    match value {
        Value::Array(values) => {
            for item in values {
                walk_json(item, base, garage, listings, excluded, depth + 1);
            }
        }
        Value::Object(map) => {
            let types = map
                .get("@type")
                .map(|value| {
                    if let Some(types) = value.as_array() {
                        types.iter().filter_map(Value::as_str).collect::<Vec<_>>()
                    } else {
                        value.as_str().into_iter().collect()
                    }
                })
                .unwrap_or_default();
            let is_vehicle = types.iter().any(|kind| {
                matches!(
                    kind.rsplit('/').next().unwrap_or(kind),
                    "Car" | "Vehicle" | "Product"
                )
            });
            if is_vehicle && map.get("offers").is_some_and(rejected_offer) {
                let raw_url = map.get("url").and_then(json_text).or_else(|| {
                    map.get("offers")
                        .and_then(|offer| offer.get("url"))
                        .and_then(json_text)
                });
                if let Some(url) = raw_url
                    .as_deref()
                    .and_then(|raw| listing_url(raw, base))
                    .or_else(|| raw_url.is_none().then(|| base.clone()))
                {
                    excluded.insert(url.to_string());
                }
            }
            if is_vehicle
                && let (Some(title), Some((price, offer_url))) = (
                    map.get("name").and_then(json_text),
                    map.get("offers").and_then(json_offer),
                )
            {
                let raw_url = map.get("url").and_then(json_text).or(offer_url);
                // Product fragments without their own URL are only safe on a
                // detail page. Multiple objects cannot all be the same URL.
                let url = raw_url
                    .as_deref()
                    .and_then(|raw| listing_url(raw, base))
                    .or_else(|| raw_url.is_none().then(|| base.clone()));
                if let Some(url) = url.filter(|_| is_vehicle_title(&title)) {
                    let mileage = map.get("mileageFromOdometer").and_then(|m| {
                        let unit = m.get("unitCode").and_then(Value::as_str).unwrap_or("KMT");
                        if !matches!(unit, "KMT" | "km" | "KM") {
                            return None;
                        }
                        json_text(m).and_then(|s| integer(&s))
                    });
                    let model_year = map
                        .get("vehicleModelDate")
                        .or_else(|| map.get("productionDate"))
                        .or_else(|| map.get("dateVehicleFirstRegistered"))
                        .and_then(json_text)
                        .and_then(|s| year(&s));
                    let fuel = map.get("fuelType").and_then(json_text);
                    listings.push(make_listing(
                        garage, title, price, url, mileage, model_year, fuel,
                    ));
                }
            }
            for child in map.values() {
                if child.is_array() || child.is_object() {
                    walk_json(child, base, garage, listings, excluded, depth + 1);
                }
            }
        }
        _ => (),
    }
}

fn card_listing(
    card: ElementRef<'_>,
    base: &Url,
    garage: &Garage,
    custom: Option<&Selectors>,
    is_detail: bool,
) -> Option<Listing> {
    let title_css = custom
        .filter(|s| !s.title.trim().is_empty())
        .map(|s| s.title.as_str())
        .unwrap_or(if is_detail {
            "h1, [itemprop='name']"
        } else {
            TITLE_SELECTORS
        });
    let title = select_text(card, title_css)?;
    if !is_vehicle_title(&title) {
        return None;
    }
    let availability = selector("[itemprop='availability'], [class*='sold'], [class*='vendu'], .availability, .disponibilite, .badge, .status").ok()?;
    if card.select(&availability).any(|element| {
        let value = element.value();
        unavailable(
            value
                .attr("href")
                .or_else(|| value.attr("content"))
                .unwrap_or(""),
        ) || unavailable(&text(element))
            || value.attr("class").is_some_and(unavailable)
    }) {
        return None;
    }
    let currency = select_text(card, "[itemprop='priceCurrency']").unwrap_or_default();
    if !currency.is_empty() && !currency.eq_ignore_ascii_case("EUR") {
        return None;
    }
    let price_css = custom
        .filter(|s| !s.price.trim().is_empty())
        .map(|s| s.price.as_str())
        .unwrap_or(PRICE_SELECTORS);
    let price_selector = selector(price_css).ok()?;
    let price = card.select(&price_selector).find_map(|element| {
        // A monthly suffix is often a sibling of the highlighted amount.
        let suffix = element
            .next_siblings()
            .filter_map(|node| {
                ElementRef::wrap(node)
                    .map(text)
                    .or_else(|| node.value().as_text().map(|value| value.text.to_string()))
            })
            .find(|value| !value.trim().is_empty())
            .unwrap_or_default();
        if financing(&suffix) {
            return None;
        }
        cash_price(&text(element), currency.eq_ignore_ascii_case("EUR"))
    })?;
    let link_css = custom
        .filter(|s| !s.link.trim().is_empty())
        .map(|s| s.link.as_str())
        .unwrap_or("a[href]");
    let link_selector = selector(link_css).ok()?;
    let link = if link_selector.matches(&card) {
        card.value().attr("href")
    } else {
        card.select(&link_selector)
            .find_map(|element| element.value().attr("href"))
    };
    let url = if is_detail {
        base.clone()
    } else {
        listing_url(link?, base)?
    };
    let mileage_css = custom.filter(|s| !s.mileage.trim().is_empty()).map(|s| s.mileage.as_str()).unwrap_or("[itemprop='mileageFromOdometer'], .mileage, .kilometrage, [class*='mileage'], [class*='kilometr']");
    let mileage = select_text(card, mileage_css).and_then(|s| integer(&s));
    let year_css = custom
        .filter(|s| !s.year.trim().is_empty())
        .map(|s| s.year.as_str())
        .unwrap_or("[itemprop='vehicleModelDate'], .year, .annee, [class*='registration']");
    let model_year = select_text(card, year_css).and_then(|s| year(&s));
    let fuel = select_text(card, "[itemprop='fuelType'], .fuel, .carburant");
    Some(make_listing(
        garage, title, price, url, mileage, model_year, fuel,
    ))
}

fn extract_listings(html: &str, base: &Url, garage: &Garage) -> Result<Vec<Listing>> {
    let document = Html::parse_document(html);
    let mut listings = Vec::new();
    let mut excluded = HashSet::new();
    for script in document.select(&selector("script[type='application/ld+json']")?) {
        if let Ok(value) = serde_json::from_str::<Value>(&script.inner_html()) {
            walk_json(&value, base, garage, &mut listings, &mut excluded, 0);
        }
    }
    if !garage.selectors.card.trim().is_empty() {
        for raw in [
            &garage.selectors.card,
            &garage.selectors.title,
            &garage.selectors.price,
            &garage.selectors.link,
            &garage.selectors.mileage,
            &garage.selectors.year,
        ] {
            if !raw.trim().is_empty() {
                selector(raw)?;
            }
        }
        for card in document.select(&selector(&garage.selectors.card)?) {
            if let Some(listing) = card_listing(card, base, garage, Some(&garage.selectors), false)
            {
                listings.push(listing);
            }
        }
    } else if listings.is_empty() {
        let cards = selector(
            "[itemtype$='/Car'], [itemtype$='/Vehicle'], [itemtype$='/Product'], .vehicle-card, .car-card, .car-item, .vehicle-item, .voiture, .vehicule, [class*='vehicle-card'], [class*='vehicle-item'], [class*='car-listing'], [class*='annonce-item'], [class*='vehicule-item'], article",
        )?;
        for card in document.select(&cards) {
            if let Some(listing) = card_listing(card, base, garage, None, false) {
                listings.push(listing);
            }
        }
        // A detail page needs a single main title and a vehicle-looking URL or
        // explicit vehicle microdata. Do not interpret a homepage's first price.
        let h1_count = document.select(&selector("h1")?).count();
        let path = base.path().to_lowercase();
        let detail_path = path.split('/').filter(|s| !s.is_empty()).count() >= 2
            || ["occasion-", "vehicule-", "voiture-", "vehicle-", "detail-"]
                .iter()
                .any(|s| path.contains(s));
        if listings.is_empty()
            && h1_count == 1
            && detail_path
            && let Some(listing) = card_listing(document.root_element(), base, garage, None, true)
        {
            listings.push(listing);
        }
    }
    listings.retain(|listing| !excluded.contains(&listing.url));
    Ok(listings)
}

fn detail_links(html: &str, base: &Url) -> Vec<Url> {
    let document = Html::parse_document(html);
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    fn json_links(value: &Value, out: &mut Vec<String>, depth: usize) {
        if depth > 32 {
            return;
        }
        match value {
            Value::Array(values) => {
                for item in values {
                    json_links(item, out, depth + 1);
                }
            }
            Value::Object(map) => {
                if map
                    .get("@type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "ListItem" | "Car" | "Vehicle" | "Product"))
                {
                    if let Some(url) = map.get("url").and_then(Value::as_str) {
                        out.push(url.to_owned());
                    }
                    if let Some(url) = map.get("item").and_then(Value::as_str) {
                        out.push(url.to_owned());
                    }
                }
                for child in map.values() {
                    json_links(child, out, depth + 1);
                }
            }
            _ => (),
        }
    }
    let mut raw_links = Vec::new();
    for script in
        document.select(&selector("script[type='application/ld+json']").expect("static selector"))
    {
        if let Ok(value) = serde_json::from_str::<Value>(&script.inner_html()) {
            json_links(&value, &mut raw_links, 0);
        }
    }
    for anchor in document.select(&selector("a[href]").expect("static selector")) {
        let href = anchor.value().attr("href").unwrap_or("");
        let lower = href.to_lowercase();
        if [
            "occasion", "vehicule", "voiture", "vehicle", "/car/", "detail",
        ]
        .iter()
        .any(|s| lower.contains(s))
        {
            raw_links.push(href.to_owned());
        }
    }
    for raw in raw_links {
        if let Some(url) = listing_url(&raw, base)
            && url != *base
            && ![".pdf", ".jpg", ".png", ".zip"]
                .iter()
                .any(|suffix| url.path().ends_with(*suffix))
            && seen.insert(url.to_string())
        {
            links.push(url);
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_server(routes: Vec<(&'static str, &'static str)>) -> (String, thread::JoinHandle<()>) {
        use std::io::Write;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            for (path, body) in routes {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 4096];
                let count = stream.read(&mut request).unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..count]).starts_with(&format!("GET {path} "))
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (address, worker)
    }

    fn garage() -> Garage {
        Garage {
            id: "garage-test".into(),
            name: "Garage indépendant".into(),
            url: "https://garage.example/occasions/".into(),
            enabled: true,
            selectors: Selectors::default(),
            last_scan: None,
            last_error: None,
        }
    }

    #[test]
    fn parses_cash_euros_without_mixing_financing() {
        assert_eq!(cash_price("12\u{202f}990,00 €", false), Some(12990));
        assert_eq!(cash_price("EUR 12,990.00", false), Some(12990));
        assert_eq!(cash_price("12.990 €", false), Some(12990));
        assert_eq!(cash_price("12990", true), Some(12990));
        assert_eq!(cash_price("12990", false), None);
        assert_eq!(cash_price("599 € /mois", false), None);
        assert_eq!(cash_price("À partir de 12 990 €", false), None);
        assert_eq!(cash_price("12 990 USD", true), None);
        assert_eq!(cash_price("12 990,99 €", false), None);
    }

    #[test]
    fn reads_jsonld_graph_and_relative_urls() {
        let garage = garage();
        let url = Url::parse(&garage.url).unwrap();
        let listings = extract_listings(
            include_str!("../tests/fixtures/garage-jsonld.html"),
            &url,
            &garage,
        )
        .unwrap();
        assert_eq!(listings.len(), 1);
        let car = &listings[0];
        assert_eq!(car.title, "Peugeot 308 BlueHDi 130");
        assert_eq!(car.price, 14990);
        assert_eq!(car.mileage, Some(52000));
        assert_eq!(car.year, Some(2020));
        assert_eq!(car.url, "https://garage.example/occasions/peugeot-308");
    }

    #[test]
    fn reads_associated_html_cards_and_rejects_financing() {
        let garage = garage();
        let listings = extract_listings(
            include_str!("../tests/fixtures/garage-cards.html"),
            &Url::parse(&garage.url).unwrap(),
            &garage,
        )
        .unwrap();
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].price, 10990);
        assert_eq!(listings[0].title, "Renault Clio TCe 90");
        assert_eq!(listings[0].year, Some(2019));
    }

    #[test]
    fn custom_selectors_are_validated_and_supported() {
        let mut garage = garage();
        garage.selectors = Selectors {
            card: ".stock".into(),
            title: ".label".into(),
            price: ".amount".into(),
            link: "a".into(),
            mileage: String::new(),
            year: String::new(),
        };
        let html = "<div class='stock'><a href='/voiture/42'><b class='label'>Toyota Yaris</b></a><span class='amount'>15 500 €</span></div>";
        assert_eq!(
            extract_listings(html, &Url::parse(&garage.url).unwrap(), &garage).unwrap()[0].price,
            15500
        );
        garage.selectors.price = "[".into();
        assert!(extract_listings(html, &Url::parse(&garage.url).unwrap(), &garage).is_err());
    }

    #[test]
    fn does_not_follow_portals_or_external_links() {
        let base = Url::parse("https://garage.example/occasions").unwrap();
        assert!(listing_url("https://leboncoin.fr/ad/123", &base).is_none());
        assert!(listing_url("https://another.example/car", &base).is_none());
        assert!(listing_url("javascript:alert(1)", &base).is_none());
        assert!(validate_url("https://www.lacentrale.fr/").is_err());
        assert!(validate_url("https://user:secret@garage.example/").is_err());
    }

    #[test]
    fn stable_identity_ignores_tracking_parameters() {
        let base = Url::parse("https://garage.example/").unwrap();
        let one = listing_url("/auto?id=23&utm_source=google#price", &base).unwrap();
        let two = listing_url("/auto?id=23", &base).unwrap();
        assert_eq!(one, two);
        assert_eq!(
            make_listing(
                &garage(),
                "Peugeot 308".into(),
                12000,
                one,
                None,
                None,
                None
            )
            .id,
            make_listing(
                &garage(),
                "Peugeot 308".into(),
                11000,
                two,
                None,
                None,
                None
            )
            .id
        );
    }

    #[test]
    fn robots_rules_and_delay_are_honored() {
        let robots = "User-agent: GarageDistiller\nDisallow: /private\nAllow: /private/public\nCrawl-delay: 2.5\n";
        let mut matcher = robotstxt::DefaultMatcher::default();
        assert!(!matcher.one_agent_allowed_by_robots(
            robots,
            AGENT,
            "https://garage.example/private/car"
        ));
        assert!(matcher.one_agent_allowed_by_robots(
            robots,
            AGENT,
            "https://garage.example/private/public/car"
        ));
        assert_eq!(crawl_delay(robots), Duration::from_millis(2500));
    }

    #[test]
    fn extracts_only_same_site_detail_candidates() {
        let base = Url::parse("https://garage.example/occasions/").unwrap();
        let html = "<a href='/occasions/clio'>Clio</a><a href='/occasions/clio'>Encore</a><a href='https://other.example/vehicule/42'>Externe</a><a href='/contact'>Contact</a>";
        let links = detail_links(html, &base);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].as_str(), "https://garage.example/occasions/clio");
    }

    #[test]
    fn scan_fetches_robots_then_extracts_real_http_response() {
        let (address, worker) = local_server(vec![
            ("/robots.txt", "User-agent: *\nAllow: /\n"),
            (
                "/occasions/",
                include_str!("../tests/fixtures/garage-jsonld.html"),
            ),
        ]);
        let mut garage = garage();
        garage.url = format!("{address}/occasions/");
        let listings = scan_garage(&garage).unwrap();
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].price, 14990);
        assert_eq!(listings[0].url, format!("{address}/occasions/peugeot-308"));
        worker.join().unwrap();
    }

    #[test]
    fn scan_never_requests_a_page_disallowed_by_robots() {
        let (address, worker) = local_server(vec![("/robots.txt", "User-agent: *\nDisallow: /\n")]);
        let mut garage = garage();
        garage.url = format!("{address}/private");
        let error = scan_garage(&garage).unwrap_err().to_string();
        assert!(error.contains("interdite par robots.txt"), "{error}");
        worker.join().unwrap();
    }

    #[test]
    fn sold_jsonld_cannot_reappear_through_html_fallback() {
        let garage = garage();
        let url = Url::parse("https://garage.example/occasions/clio").unwrap();
        let html = r#"<script type="application/ld+json">{"@type":"Car","name":"Renault Clio","offers":{"price":12000,"priceCurrency":"EUR","availability":"https://schema.org/SoldOut"}}</script><h1>Renault Clio</h1><span class="price">12 000 €</span>"#;
        assert!(extract_listings(html, &url, &garage).unwrap().is_empty());
    }

    #[test]
    fn html_sold_badge_and_monthly_sibling_are_rejected() {
        let garage = garage();
        let url = Url::parse(&garage.url).unwrap();
        for marker in [
            "<span class='status'>Vendu</span><span class='price'>12 000 €</span>",
            "<span class='price'>599 €</span><span>/mois</span>",
        ] {
            let html = format!(
                "<article><a href='/occasions/clio'><h2>Renault Clio</h2></a>{marker}</article>"
            );
            assert!(extract_listings(&html, &url, &garage).unwrap().is_empty());
        }
    }

    #[test]
    fn accessory_titles_and_recurring_price_arrays_are_rejected() {
        assert!(!is_vehicle_title("Renault Clio — pièces détachées"));
        assert!(!is_vehicle_title("Peugeot 208 accessoires"));
        assert!(is_vehicle_title("Renault Clio TCe 90 Intens"));
        let offer = serde_json::json!({"price":599,"priceCurrency":"EUR","priceSpecification":[{"billingDuration":"P1M"}]});
        assert!(json_offer(&offer).is_none());
    }
}
