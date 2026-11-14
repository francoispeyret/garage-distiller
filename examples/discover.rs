//! Manual live smoke test: cargo run --example discover -- Renault Clio 12000 [--scan]
//! Makes real search requests. --scan additionally reads the first three sources.

use garage_distiller::{collector, discovery, model::Search};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 || args.iter().any(|arg| arg == "--help") {
        println!("Usage: cargo run --example discover -- MARQUE MODELE BUDGET [--scan]");
        println!("Exemple: cargo run --example discover -- Renault 'Clio IV' 12000 --scan");
        println!("Clé facultative : variable GARAGE_DISTILLER_BRAVE_API_KEY.");
        return Ok(());
    }
    let search = Search {
        id: "smoke-test".into(),
        make: args[0].clone(),
        model: args[1].clone(),
        max_price: args[2].parse()?,
        filters: Default::default(),
        enabled: true,
        created_at: chrono::Utc::now().timestamp(),
    };
    let key = std::env::var("GARAGE_DISTILLER_BRAVE_API_KEY").ok();
    let started = std::time::Instant::now();
    let result = discovery::discover(&search, key.as_deref())?;
    println!(
        "{} source(s) trouvée(s) en {:.1}s",
        result.garages.len(),
        started.elapsed().as_secs_f32()
    );
    for warning in &result.warnings {
        println!("AVERTISSEMENT : {warning}");
    }
    for source in &result.garages {
        println!("{} | {}", source.name, source.url);
    }
    if args.iter().any(|arg| arg == "--scan") {
        let mut scanned = 0;
        let mut listings = 0;
        let mut matching = 0;
        let mut errors = 0;
        for garage in result.garages.iter().take(3) {
            scanned += 1;
            match collector::scan_garage(garage) {
                Ok(found) => {
                    listings += found.len();
                    for listing in found {
                        let matches = search.matches(&listing);
                        if matches {
                            matching += 1;
                        }
                        println!(
                            "ANNONCE [{}] {} | {} EUR | {}",
                            if matches {
                                "correspond"
                            } else {
                                "hors critères"
                            },
                            listing.title,
                            listing.price,
                            listing.url
                        );
                    }
                }
                Err(error) => {
                    errors += 1;
                    println!("ERREUR {} : {error:#}", garage.url);
                }
            }
        }
        println!(
            "BILAN : {scanned} source(s) analysée(s), {listings} annonce(s) extraite(s), {matching} correspondance(s), {errors} erreur(s)."
        );
    }
    Ok(())
}
