use eframe::egui::{self, Align, Color32, FontId, Frame, Layout, Margin, RichText, Stroke, Vec2};
use egui_extras::{Column, TableBuilder};
use garage_distiller::{
    engine::{self, Event},
    model::*,
    storage, updater,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

const BG: Color32 = Color32::from_rgb(12, 15, 20);
const PANEL: Color32 = Color32::from_rgb(20, 25, 32);
const LINE: Color32 = Color32::from_rgb(38, 45, 54);
const TEXT: Color32 = Color32::from_rgb(237, 242, 246);
const MUTED: Color32 = Color32::from_rgb(139, 151, 166);
const ACCENT: Color32 = Color32::from_rgb(199, 244, 113);
const AMBER: Color32 = Color32::from_rgb(244, 192, 115);

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Overview,
    Listings,
    Searches,
    Garages,
    Alerts,
    Settings,
}

enum UpdateEvent {
    Checked(Result<Option<updater::ReleaseInfo>, String>),
    Installed(Result<(), String>),
}

pub struct GarageApp {
    state: State,
    path: Option<PathBuf>,
    read_only: bool,
    demo: bool,
    page: Page,
    receiver: Option<Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    scanning: bool,
    scan_queued: bool,
    progress: String,
    fraction: f32,
    warnings: Vec<String>,
    next_scan: Instant,
    update_rx: Option<Receiver<UpdateEvent>>,
    release: Option<updater::ReleaseInfo>,
    update_status: String,
    next_update: Instant,
    updated: bool,
    installing: bool,
    brave_api_key: String,
    notification_tx: mpsc::Sender<String>,
    notification_rx: Receiver<String>,
    toast: Option<(String, Instant)>,
    storage_error: Option<String>,
    query: String,
    favorites_only: bool,
    matching_only: bool,
    show_old: bool,
    price_sort: bool,
    listing_filters: VehicleFilters,
    search_modal: bool,
    edit_id: Option<String>,
    form_make: String,
    form_model: String,
    form_budget: u32,
    form_filters: VehicleFilters,
    detail: Option<String>,
    garage_url: String,
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    started: Instant,
}

impl GarageApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);
        let args: Vec<String> = std::env::args().collect();
        let demo = args.iter().any(|a| a == "--demo");
        let screenshot = args
            .windows(2)
            .find(|w| w[0] == "--screenshot")
            .map(|w| PathBuf::from(&w[1]));
        let path_result = storage::data_path();
        let (path, state, storage_error) = if demo {
            (
                None,
                if args.iter().any(|a| a == "--empty") {
                    State::default()
                } else {
                    demo_state()
                },
                None,
            )
        } else {
            match path_result {
                Ok(path) => match storage::load(&path) {
                    Ok(state) => (Some(path), state, None),
                    Err(e) => (Some(path), State::default(), Some(format!("{e:#}"))),
                },
                Err(e) => (None, State::default(), Some(format!("{e:#}"))),
            }
        };
        let (notification_tx, notification_rx) = mpsc::channel();
        let mut app = Self {
            read_only: storage_error.is_some(),
            path,
            state,
            storage_error,
            demo,
            page: Page::Overview,
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            scanning: false,
            scan_queued: false,
            progress: String::new(),
            fraction: 0.0,
            warnings: vec![],
            next_scan: Instant::now() + Duration::from_secs(3),
            update_rx: None,
            release: None,
            update_status: "Vérification au démarrage…".into(),
            next_update: Instant::now(),
            updated: false,
            installing: false,
            brave_api_key: std::env::var("GARAGE_DISTILLER_BRAVE_API_KEY").unwrap_or_default(),
            notification_tx,
            notification_rx,
            toast: None,
            query: String::new(),
            favorites_only: false,
            matching_only: true,
            show_old: false,
            price_sort: false,
            listing_filters: VehicleFilters::default(),
            search_modal: false,
            edit_id: None,
            form_make: String::new(),
            form_model: String::new(),
            form_budget: 15000,
            form_filters: VehicleFilters::default(),
            detail: None,
            garage_url: String::new(),
            screenshot,
            screenshot_requested: false,
            started: Instant::now(),
        };
        if demo {
            app.update_status = "Mode démonstration".into();
            if let Some(screen) = args
                .windows(2)
                .find(|w| w[0] == "--screen")
                .map(|w| w[1].as_str())
            {
                app.page = match screen {
                    "listings" => Page::Listings,
                    "searches" => Page::Searches,
                    "garages" => Page::Garages,
                    "alerts" => Page::Alerts,
                    "settings" => Page::Settings,
                    _ => Page::Overview,
                };
                if screen == "new-search" {
                    app.new_search();
                }
            }
        } else {
            app.check_update();
        }
        app
    }

    fn save(&mut self) {
        if self.demo || self.read_only {
            return;
        }
        if let Some(path) = &self.path {
            match storage::save(path, &self.state) {
                Ok(()) => self.storage_error = None,
                Err(e) => self.storage_error = Some(format!("Enregistrement impossible : {e:#}")),
            }
        }
    }

    fn message(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), Instant::now()));
    }

    fn new_search(&mut self) {
        self.edit_id = None;
        self.form_make.clear();
        self.form_model.clear();
        self.form_budget = 15000;
        self.form_filters = VehicleFilters::default();
        self.search_modal = true;
    }

    fn start_scan(&mut self) {
        if self.scanning {
            return;
        }
        if self.demo {
            self.message("La démonstration utilise des annonces fictives. Relancez sans --demo pour rechercher.");
            return;
        }
        if !self.state.searches.iter().any(|s| s.enabled) {
            self.new_search();
            return;
        }
        self.scanning = true;
        self.fraction = 0.0;
        self.warnings.clear();
        self.progress = "Préparation de la recherche…".into();
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let searches = self.state.searches.clone();
        let garages = self.state.garages.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        let key =
            (!self.brave_api_key.trim().is_empty()).then(|| self.brave_api_key.trim().to_owned());
        std::thread::spawn(move || engine::run(searches, garages, key, tx, cancel));
    }

    fn check_update(&mut self) {
        if self.demo || self.update_rx.is_some() || self.updated {
            return;
        }
        self.next_update = Instant::now() + Duration::from_secs(24 * 3600);
        self.update_status = "Vérification sur GitHub…".into();
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(UpdateEvent::Checked(
                updater::check().map_err(|e| format!("{e:#}")),
            ));
        });
    }

    fn install_update(&mut self) {
        let Some(release) = self.release.clone() else {
            return;
        };
        if self.update_rx.is_some() {
            return;
        }
        self.installing = true;
        self.update_status = format!(
            "Téléchargement et vérification de la version {}…",
            release.version
        );
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(UpdateEvent::Installed(
                updater::install(&release).map_err(|e| format!("{e:#}")),
            ));
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.receiver {
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let finished = events
            .iter()
            .any(|event| matches!(event, Event::Finished { .. }));
        let mut dirty = false;
        for event in events {
            match event {
                Event::Progress(message) => self.progress = message,
                Event::Warning(message) => self.warnings.push(message),
                Event::Discovered(garages) => {
                    for garage in garages {
                        if !self.state.garages.iter().any(|g| g.id == garage.id) {
                            self.state.garages.push(garage);
                        }
                    }
                    dirty = true;
                }
                Event::Site {
                    garage,
                    listings,
                    done,
                    total,
                } => {
                    self.fraction = done as f32 / total.max(1) as f32;
                    self.progress = format!("{done} / {total} pages de garages analysées");
                    if let Some(error) = &garage.last_error {
                        self.warnings.push(format!("{} : {}", garage.name, error));
                    }
                    if let Some(old) = self.state.garages.iter_mut().find(|g| g.id == garage.id) {
                        old.last_scan = garage.last_scan;
                        old.last_error = garage.last_error;
                    }
                    let alerts = self.state.merge_listings(listings);
                    if !alerts.is_empty() {
                        self.message(format!(
                            "{} nouvelle(s) offre(s) dans votre budget",
                            alerts.len()
                        ));
                        if self.state.settings.notifications {
                            let body = if alerts.len() == 1 {
                                format!("{} — {}", alerts[0].title, alerts[0].body)
                            } else {
                                format!(
                                    "{} nouvelles offres correspondent à vos recherches.",
                                    alerts.len()
                                )
                            };
                            // Notification services may block; keep them away from the render thread.
                            self.notify(body);
                        }
                    }
                    dirty = true;
                }
                Event::Finished { cancelled } => {
                    self.scanning = false;
                    self.receiver = None;
                    self.next_scan = Instant::now()
                        + Duration::from_secs(self.state.settings.scan_minutes.max(15) as u64 * 60);
                    if !cancelled {
                        self.state.last_scan = Some(chrono::Utc::now().timestamp());
                    }
                    self.progress = if cancelled {
                        "Recherche arrêtée"
                    } else {
                        "Recherche terminée"
                    }
                    .into();
                    if !self.warnings.is_empty() {
                        self.message(format!(
                            "Recherche terminée · {} information(s) à consulter dans Garages",
                            self.warnings.len()
                        ));
                    }
                    if self.scan_queued {
                        self.scan_queued = false;
                        self.next_scan = Instant::now();
                        self.start_scan();
                    }
                    dirty = true;
                }
            }
        }
        if disconnected && !finished && self.scanning {
            self.scanning = false;
            self.receiver = None;
            self.next_scan = Instant::now() + Duration::from_secs(900);
            self.message("L’exploration s’est interrompue. Vous pouvez la relancer.");
            if self.scan_queued {
                self.scan_queued = false;
                self.start_scan();
            }
        }
        if dirty {
            self.save();
        }
        let update_event = self.update_rx.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(UpdateEvent::Installed(Err(
                "Le service de mise à jour s’est interrompu. Relancez la vérification.".into(),
            ))),
        });
        if let Some(event) = update_event {
            self.update_rx = None;
            match event {
                UpdateEvent::Checked(Ok(Some(release))) => {
                    self.update_status = format!("Version {} disponible", release.version);
                    self.release = Some(release);
                    if self.state.settings.auto_update && updater::configured() {
                        self.install_update();
                    }
                }
                UpdateEvent::Checked(Ok(None)) => {
                    self.update_status =
                        "Aucune version plus récente publiée pour le moment.".into()
                }
                UpdateEvent::Checked(Err(error)) | UpdateEvent::Installed(Err(error)) => {
                    self.installing = false;
                    self.update_status = error;
                }
                UpdateEvent::Installed(Ok(())) => {
                    self.installing = false;
                    self.updated = true;
                    self.update_status =
                        "Mise à jour installée. Fermez puis relancez l’application.".into();
                }
            }
        }
        let notification_errors: Vec<_> = self.notification_rx.try_iter().collect();
        for error in notification_errors {
            self.warnings.push(error.clone());
            self.message(error);
        }
        if !self.demo && !self.read_only {
            if self.state.settings.auto_scan
                && !self.scanning
                && Instant::now() >= self.next_scan
                && self.state.searches.iter().any(|s| s.enabled)
            {
                self.start_scan();
            }
            if Instant::now() >= self.next_update {
                self.check_update();
            }
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, time)| time.elapsed().as_secs() > 9)
        {
            self.toast = None;
        }
        ctx.request_repaint_after(Duration::from_millis(if self.scanning {
            250
        } else {
            1000
        }));
    }

    fn notify(&self, body: String) {
        if self.demo {
            return;
        }
        let tx = self.notification_tx.clone();
        std::thread::spawn(move || {
            if let Err(error) = notify_rust::Notification::new()
                .summary("Garage Distiller · Bonne affaire repérée")
                .body(&body)
                .show()
            {
                let _=tx.send(format!("Notification système indisponible : {error}. Les détections restent visibles dans l’application."));
            }
        });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .exact_width(224.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(15, 19, 25))
                    .inner_margin(Margin::symmetric(18, 28))
                    .stroke(Stroke::new(1.0_f32, LINE)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(38.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 12, ACCENT);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "gd",
                        FontId::proportional(23.0),
                        BG,
                    );
                    ui.vertical(|ui| {
                        ui.label(RichText::new("garage distiller").strong().size(18.0));
                        ui.label(
                            RichText::new("DIRECT À LA SOURCE")
                                .size(9.0)
                                .color(MUTED)
                                .extra_letter_spacing(1.6),
                        );
                    });
                });
                ui.add_space(46.0);
                ui.label(
                    RichText::new("VOTRE ESPACE")
                        .size(10.0)
                        .color(MUTED)
                        .extra_letter_spacing(1.4),
                );
                ui.add_space(12.0);
                let unread = self.state.alerts.iter().filter(|a| !a.read).count();
                for (page, label) in [
                    (Page::Overview, "Vue d’ensemble"),
                    (Page::Listings, "Les annonces"),
                    (Page::Searches, "Mes recherches"),
                    (Page::Garages, "Garages explorés"),
                    (Page::Alerts, "Détections"),
                ] {
                    let selected = self.page == page;
                    let label = if page == Page::Alerts && unread > 0 {
                        format!("{label}   {unread}")
                    } else {
                        label.to_string()
                    };
                    if nav_button(ui, &label, page, selected).clicked() {
                        self.page = page;
                    }
                    ui.add_space(5.0);
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "v{}  ·  macOS & Windows",
                            env!("CARGO_PKG_VERSION")
                        ))
                        .size(10.0)
                        .color(MUTED),
                    );
                    ui.add_space(18.0);
                    if ui
                        .add(
                            egui::Button::new(RichText::new("⚙    Réglages").color(MUTED))
                                .frame(false)
                                .min_size(Vec2::new(180.0, 38.0)),
                        )
                        .clicked()
                    {
                        self.page = Page::Settings;
                    }
                    ui.add_space(20.0);
                    card().show(ui, |ui| {
                        ui.set_width(154.0);
                        ui.allocate_ui_with_layout(
                            Vec2::new(154.0, 95.0),
                            Layout::top_down(Align::LEFT),
                            |ui| {
                                ui.label(
                                    RichText::new(if self.scanning {
                                        "Exploration en cours"
                                    } else if self.state.settings.auto_scan {
                                        "Veille activée"
                                    } else {
                                        "Veille en pause"
                                    })
                                    .color(ACCENT)
                                    .size(12.0),
                                );
                                ui.add_space(8.0);
                                ui.label(
                            RichText::new(format!(
                                "Un passage toutes les {} min tant que l’application est ouverte.",
                                self.state.settings.scan_minutes
                            ))
                            .color(MUTED)
                            .size(11.0),
                        );
                            },
                        );
                    });
                });
            });
    }

    fn header(&mut self, ui: &mut egui::Ui, title: &str, subtitle: &str) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(RichText::new(title).size(29.0).strong());
                ui.add_space(5.0);
                ui.label(RichText::new(subtitle).color(MUTED).size(13.0));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary(ui, "+  Nouvelle recherche").clicked() {
                    self.new_search();
                }
            });
        });
        ui.add_space(18.0);
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        self.header(
            ui,
            "Une longueur d’avance.",
            "Les occasions des garages, repérées directement à la source.",
        );
        Frame::new().fill(Color32::from_rgb(27,36,29)).stroke(Stroke::new(1.0_f32,Color32::from_rgb(54,68,43))).corner_radius(14).inner_margin(18).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("LE BON MODÈLE. AU BON PRIX.").color(ACCENT).size(10.0).extra_letter_spacing(1.8));
                    ui.add_space(10.0);
                    ui.label(RichText::new("Vous choisissez la voiture. On explore les garages.").size(20.0).strong());
                    ui.add_space(7.0);
                    ui.label(RichText::new("Recherche web automatique · Sites de professionnels · Alertes sur votre budget").color(MUTED).size(12.0));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.scanning { if secondary(ui,"Arrêter").clicked() { self.cancel.store(true,Ordering::Relaxed); self.progress = "Arrêt après les pages en cours…".into(); } }
                    else if primary(ui,"Explorer maintenant  ↗").clicked() { self.start_scan(); }
                });
            });
        });
        ui.add_space(20.0);
        let matches = self
            .state
            .listings
            .iter()
            .filter(|l| {
                l.last_seen >= chrono::Utc::now().timestamp() - 172_800
                    && self.state.searches.iter().any(|s| s.matches(l))
            })
            .count();
        let sites = self
            .state
            .garages
            .iter()
            .filter_map(|g| {
                url::Url::parse(&g.url)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned))
            })
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let active = self.state.searches.iter().filter(|s| s.enabled).count();
        let unread = self.state.alerts.iter().filter(|a| !a.read).count();
        ui.columns(4, |columns| {
            for (ui, (value, label, foot)) in columns.iter_mut().zip([
                (
                    matches,
                    "Offres dans le budget",
                    "Selon vos recherches actives",
                ),
                (sites, "Sites découverts", "Garages & concessions"),
                (active, "Recherches actives", "Vos modèles à surveiller"),
                (unread, "Nouvelles détections", "À consulter"),
            ]) {
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(label).color(MUTED).size(12.0));
                    ui.add_space(6.0);
                    ui.label(RichText::new(value.to_string()).size(30.0).strong());
                    ui.label(RichText::new(foot).color(MUTED).size(10.0));
                });
            }
        });
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Votre sélection").size(19.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.link("Toutes les annonces").clicked() {
                    self.page = Page::Listings;
                }
            });
        });
        ui.add_space(12.0);
        self.listing_table(ui, true);
    }

    fn listing_table(&mut self, ui: &mut egui::Ui, compact: bool) {
        if !compact {
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Filtrer par modèle ou garage…")
                        .desired_width(270.0),
                );
                ui.add_space(12.0);
                ui.checkbox(&mut self.matching_only, "Dans mon budget");
                ui.checkbox(&mut self.favorites_only, "Favoris");
                ui.checkbox(&mut self.show_old, "Archives")
                    .on_hover_text("Inclure les annonces non revues depuis plus de 48 heures");
                ui.checkbox(&mut self.price_sort, "Prix croissant");
            });
            ui.add_space(8.0);
            vehicle_filters(ui, &mut self.listing_filters);
            ui.add_space(16.0);
        }
        let query = normalize(&self.query);
        let mut listings: Vec<Listing> = self
            .state
            .listings
            .iter()
            .filter(|l| {
                (!self.favorites_only || l.favorite)
                    && self.listing_filters.matches(l)
                    && (self.show_old
                        || self.favorites_only
                        || l.last_seen >= chrono::Utc::now().timestamp() - 172_800)
                    && (!self.matching_only || self.state.searches.iter().any(|s| s.matches(l)))
                    && (query.is_empty()
                        || normalize(&format!("{} {}", l.title, l.garage_name)).contains(&query))
            })
            .cloned()
            .collect();
        if self.price_sort {
            listings.sort_by_key(|l| l.price);
        } else {
            listings.sort_by_key(|l| std::cmp::Reverse(l.first_seen));
        }
        if compact {
            listings.truncate(4);
        }
        if listings.is_empty() {
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(32.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("+").size(40.0).color(ACCENT));
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(if self.state.searches.is_empty() {
                            "Votre prochaine bonne affaire commence ici."
                        } else if self.scanning {
                            "Nous explorons les sites des garages…"
                        } else {
                            "Aucune annonce pour ces critères, pour le moment."
                        })
                        .size(18.0)
                        .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Choisissez un modèle et un budget. Les résultats apparaîtront ici.",
                        )
                        .color(MUTED),
                    );
                    ui.add_space(20.0);
                    if primary(
                        ui,
                        if self.state.searches.is_empty() {
                            "+  Créer ma première recherche"
                        } else {
                            "Relancer l’exploration"
                        },
                    )
                    .clicked()
                    {
                        self.start_scan();
                    }
                });
                ui.add_space(36.0);
            });
            return;
        }
        let mut favorite = None;
        let mut detail = None;
        Frame::new()
            .fill(PANEL)
            .corner_radius(12)
            .stroke(Stroke::new(1.0_f32, LINE))
            .inner_margin(12)
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .vscroll(!compact)
                    .striped(true)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::remainder().at_least(210.0))
                    .column(Column::initial(170.0).at_least(110.0))
                    .column(Column::exact(105.0))
                    .column(Column::exact(110.0))
                    .column(Column::exact(100.0))
                    .column(Column::exact(50.0))
                    .header(34.0, |mut row| {
                        for name in ["VÉHICULE", "GARAGE", "KILOMÉTRAGE", "PRIX", "REPÉRÉE", " "]
                        {
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(name)
                                        .size(10.0)
                                        .color(MUTED)
                                        .extra_letter_spacing(0.6),
                                );
                            });
                        }
                    })
                    .body(|mut body| {
                        for listing in &listings {
                            body.row(64.0, |mut row| {
                                row.col(|ui| {
                                    ui.vertical(|ui| {
                                        ui.add_space(13.0);
                                        if ui
                                            .add(
                                                egui::Label::new(
                                                    RichText::new(&listing.title)
                                                        .strong()
                                                        .size(14.0),
                                                )
                                                .truncate()
                                                .sense(egui::Sense::click()),
                                            )
                                            .on_hover_text(&listing.title)
                                            .clicked()
                                        {
                                            detail = Some(listing.id.clone());
                                        }
                                        ui.label(
                                            RichText::new(format!(
                                                "{}  ·  {}",
                                                listing
                                                    .year
                                                    .map(|y| y.to_string())
                                                    .unwrap_or_else(|| "Année non précisée".into()),
                                                listing.fuel.as_deref().unwrap_or("Occasion")
                                            ))
                                            .size(11.0)
                                            .color(MUTED),
                                        );
                                    });
                                });
                                row.col(|ui| {
                                    ui.vertical(|ui| {
                                        ui.add_space(13.0);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&listing.garage_name).size(12.0),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(&listing.garage_name);
                                        ui.label(
                                            RichText::new("Site du professionnel")
                                                .color(MUTED)
                                                .size(10.0),
                                        );
                                    });
                                });
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(
                                            listing
                                                .mileage
                                                .map(|km| format!("{} km", number(km)))
                                                .unwrap_or_else(|| "—".into()),
                                        )
                                        .size(12.0),
                                    );
                                });
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(format!("{} €", number(listing.price)))
                                            .color(ACCENT)
                                            .strong()
                                            .size(16.0),
                                    );
                                });
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(relative(listing.first_seen))
                                            .color(MUTED)
                                            .size(11.0),
                                    );
                                });
                                row.col(|ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(if listing.favorite {
                                                    "★"
                                                } else {
                                                    "☆"
                                                })
                                                .color(if listing.favorite {
                                                    ACCENT
                                                } else {
                                                    MUTED
                                                })
                                                .size(22.0),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text("Conserver dans les favoris")
                                        .clicked()
                                    {
                                        favorite = Some(listing.id.clone());
                                    }
                                });
                            });
                        }
                    });
            });
        if let Some(id) = favorite {
            if let Some(listing) = self.state.listings.iter_mut().find(|l| l.id == id) {
                listing.favorite = !listing.favorite;
            }
            self.save();
        }
        if detail.is_some() {
            self.detail = detail;
        }
        ui.add_space(12.0);
        ui.label(RichText::new("Prix relevés sur les sites des garages. Disponibilité à confirmer auprès du vendeur.").color(MUTED).size(11.0));
    }

    fn searches(&mut self, ui: &mut egui::Ui) {
        self.header(
            ui,
            "Mes recherches",
            "Un modèle, un budget : la découverte des garages est automatique.",
        );
        let mut remove = None;
        let mut edit = None;
        let mut dirty = false;
        for search in &mut self.state.searches {
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(search.label()).strong().size(19.0));
                        ui.label(
                            RichText::new(format!(
                                "Jusqu’à {} € · France · sites de garages",
                                number(search.max_price)
                            ))
                            .color(MUTED),
                        );
                        let criteria = search.filters.description();
                        if !criteria.is_empty() {
                            ui.label(RichText::new(criteria).color(ACCENT).size(12.0));
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if secondary(ui, "Supprimer").clicked() {
                            remove = Some(search.id.clone());
                        }
                        if secondary(ui, "Modifier").clicked() {
                            edit = Some(search.clone());
                        }
                        dirty |= ui.checkbox(&mut search.enabled, "Active").changed();
                    });
                });
            });
            ui.add_space(12.0);
        }
        if self.state.searches.is_empty() {
            ui.label(RichText::new("Aucune recherche enregistrée. Créez votre première veille avec le bouton ci-dessus.").color(MUTED));
        }
        if let Some(id) = remove {
            self.state.searches.retain(|s| s.id != id);
            dirty = true;
        }
        if let Some(search) = edit {
            self.form_make = search.make;
            self.form_model = search.model;
            self.form_budget = search.max_price;
            self.form_filters = search.filters;
            self.edit_id = Some(search.id);
            self.search_modal = true;
        }
        if dirty {
            self.save();
        }
    }

    fn garages(&mut self, ui: &mut egui::Ui) {
        self.header(
            ui,
            "Garages explorés",
            "Les pages découvertes automatiquement, et le résultat de leur dernière lecture.",
        );
        ui.collapsing("Ajouter un site en complément", |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.garage_url)
                        .desired_width(400.0)
                        .hint_text("https://www.garage.fr/voitures-occasion"),
                );
                if secondary(ui, "Ajouter").clicked() {
                    match url::Url::parse(self.garage_url.trim()) {
                        Ok(url)
                            if ["https", "http"].contains(&url.scheme())
                                && url.host_str().is_some()
                                && url.username().is_empty()
                                && url.password().is_none() =>
                        {
                            if !self.state.garages.iter().any(|g| g.url == url.as_str()) {
                                self.state.garages.push(Garage {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    name: url
                                        .host_str()
                                        .unwrap_or("Garage")
                                        .trim_start_matches("www.")
                                        .into(),
                                    url: url.to_string(),
                                    enabled: true,
                                    selectors: Selectors::default(),
                                    last_scan: None,
                                    last_error: None,
                                });
                                self.save();
                            }
                            self.garage_url.clear();
                        }
                        _ => self.message("Entrez une adresse de site http ou https valide."),
                    }
                }
            });
        });
        ui.add_space(15.0);
        if !self.warnings.is_empty() {
            ui.collapsing(
                format!(
                    "{} informations sur la dernière exploration",
                    self.warnings.len()
                ),
                |ui| {
                    for warning in &self.warnings {
                        ui.label(RichText::new(warning).color(AMBER).size(12.0));
                    }
                },
            );
            ui.add_space(15.0);
        }
        let mut dirty = false;
        for garage in &mut self.state.garages {
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&garage.name).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        dirty |= ui.checkbox(&mut garage.enabled, "Surveiller").changed();
                    });
                });
                ui.add(
                    egui::Label::new(RichText::new(&garage.url).color(MUTED).size(11.0)).truncate(),
                )
                .on_hover_text(&garage.url);
                ui.add_space(6.0);
                if let Some(error) = &garage.last_error {
                    ui.label(RichText::new(error).size(12.0).color(AMBER));
                } else {
                    ui.label(
                        RichText::new(
                            garage
                                .last_scan
                                .map(|t| format!("Lecture réussie · {}", relative(t)))
                                .unwrap_or_else(|| "En attente de lecture".into()),
                        )
                        .color(ACCENT)
                        .size(12.0),
                    );
                }
                ui.collapsing("Adapter l’extraction à ce site", |ui| {
                    ui.label(
                        RichText::new(
                            "Facultatif : sélecteurs CSS pour les sites sans données structurées.",
                        )
                        .color(MUTED)
                        .size(11.0),
                    );
                    egui::Grid::new(format!("selectors_{}", garage.id))
                        .num_columns(2)
                        .show(ui, |ui| {
                            for (label, value) in [
                                ("Carte véhicule", &mut garage.selectors.card),
                                ("Titre", &mut garage.selectors.title),
                                ("Prix comptant", &mut garage.selectors.price),
                                ("Lien", &mut garage.selectors.link),
                                ("Kilométrage", &mut garage.selectors.mileage),
                                ("Année", &mut garage.selectors.year),
                            ] {
                                ui.label(label);
                                dirty |= ui.text_edit_singleline(value).changed();
                                ui.end_row();
                            }
                        });
                });
            });
            ui.add_space(10.0);
        }
        if self.state.garages.is_empty() {
            ui.label(
                RichText::new(
                    "Lancez une recherche : les sites trouvés sur le web apparaîtront ici.",
                )
                .color(MUTED),
            );
        }
        if dirty {
            self.save();
        }
    }

    fn alerts(&mut self, ui: &mut egui::Ui) {
        self.header(
            ui,
            "Les bonnes nouvelles",
            "Une détection lorsqu’une nouvelle offre correspond à votre recherche.",
        );
        if secondary(ui, "Tout marquer comme lu").clicked() {
            for alert in &mut self.state.alerts {
                alert.read = true;
            }
            self.save();
        }
        ui.add_space(16.0);
        let mut selected = None;
        for alert in self.state.alerts.iter().rev() {
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}  {}",
                                if alert.read { "○" } else { "●" },
                                alert.title
                            ))
                            .strong()
                            .color(if alert.read {
                                TEXT
                            } else {
                                ACCENT
                            }),
                        );
                        ui.label(RichText::new(&alert.body).color(MUTED));
                        ui.label(
                            RichText::new(relative(alert.created_at))
                                .color(MUTED)
                                .size(10.0),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if secondary(ui, "Voir l’annonce  ↗").clicked() {
                            selected = Some((alert.id.clone(), alert.listing_id.clone()));
                        }
                    });
                });
            });
            ui.add_space(10.0);
        }
        if self.state.alerts.is_empty() {
            ui.label(RichText::new("Les nouvelles offres dans votre budget vous seront signalées ici et par notification.").color(MUTED));
        }
        if let Some((id, listing)) = selected {
            if let Some(alert) = self.state.alerts.iter_mut().find(|a| a.id == id) {
                alert.read = true;
            }
            self.detail = Some(listing);
            self.save();
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        self.header(ui, "Réglages", "Une veille qui s’adapte à vous.");
        let mut dirty = false;
        card().show(ui, |ui| {
            ui.set_width(ui.available_width()); ui.label(RichText::new("Exploration & notifications").size(18.0).strong()); ui.add_space(15.0);
            dirty |= ui.checkbox(&mut self.state.settings.auto_scan,"Explorer automatiquement tant que l’application est ouverte").changed();
            ui.horizontal(|ui| {ui.label("Intervalle entre deux explorations"); dirty |= ui.add(egui::DragValue::new(&mut self.state.settings.scan_minutes).range(15..=1440).suffix(" min")).changed();});
            dirty |= ui.checkbox(&mut self.state.settings.notifications,"Recevoir les notifications du système").changed();
            ui.label(RichText::new(if cfg!(target_os="windows") {"Cette version portable utilise le service Windows PowerShell pour les notifications. Autorisez-les dans les réglages Windows."} else {"Autorisez les notifications de Garage Distiller dans les réglages de macOS."}).color(MUTED).size(11.0));
            if secondary(ui,"Tester les notifications").clicked() {self.notify("Votre veille est prête. Les nouvelles offres seront signalées ici.".into());}
            ui.add_space(14.0);
            ui.label(RichText::new("Découverte web").strong());
            let api = !self.brave_api_key.trim().is_empty();
            ui.label(if api {"API Brave Search configurée"} else {"Recherche web publique · aucune clé nécessaire"});
            ui.label(RichText::new("Les moteurs peuvent limiter les requêtes. Une API Brave Search peut être configurée au lancement pour fiabiliser la découverte.").color(MUTED).size(12.0));
            ui.add_space(8.0);
            ui.add(egui::TextEdit::singleline(&mut self.brave_api_key).password(true).hint_text("Clé API Brave Search · facultative").desired_width(400.0));
            ui.label(RichText::new("Clé gardée uniquement en mémoire pour cette session.").color(MUTED).size(11.0));
            ui.hyperlink_to("Configurer une clé Brave Search  ↗","https://api-dashboard.search.brave.com/");
        });
        ui.add_space(16.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width()); ui.label(RichText::new("Mises à jour GitHub").size(18.0).strong()); ui.add_space(12.0);
            ui.label(format!("Version installée : {}",env!("CARGO_PKG_VERSION")));
            dirty |= ui.checkbox(&mut self.state.settings.auto_update,"Installer automatiquement les mises à jour signées").changed();
            ui.label(RichText::new(&self.update_status).color(if self.updated {ACCENT} else {MUTED}));
            if !updater::configured() { ui.label(RichText::new("Cette compilation de développement n’intègre pas encore de clé de mise à jour. Les versions distribuées par GitHub utilisent une signature vérifiée.").size(12.0).color(AMBER)); }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(self.update_rx.is_none(),egui::Button::new("Vérifier maintenant").min_size(Vec2::new(170.0,38.0))).clicked() { self.check_update(); }
                if self.release.is_some() && !self.updated && ui.add_enabled(updater::configured() && self.update_rx.is_none(),egui::Button::new("Installer la mise à jour").min_size(Vec2::new(180.0,38.0))).clicked() { self.install_update(); }
            });
            if let Some(release)=&self.release {ui.collapsing("Notes de version",|ui|{ui.label(&release.notes);});}
            ui.hyperlink_to("Ouvrir les versions sur GitHub  ↗",updater::RELEASES_URL);
        });
        ui.add_space(16.0);
        card().show(ui, |ui| { ui.set_width(ui.available_width()); ui.label(RichText::new("Vos données restent sur cet ordinateur").strong()); ui.add_space(8.0); ui.label(RichText::new("Recherches, favoris, annonces et historique sont enregistrés localement. Les termes recherchés sont envoyés au moteur de recherche ; les sites des garages sont consultés directement.").color(MUTED)); if let Some(path)=&self.path { ui.add_space(8.0); if secondary(ui,"Ouvrir le dossier de données").clicked() && let Some(parent)=path.parent() { let _=open::that(parent); } } });
        if dirty {
            self.next_scan =
                Instant::now() + Duration::from_secs(self.state.settings.scan_minutes as u64 * 60);
            self.save();
        }
    }

    fn search_dialog(&mut self, ctx: &egui::Context) {
        if !self.search_modal {
            return;
        }
        let mut opened = true;
        let mut submit = false;
        egui::Window::new(if self.edit_id.is_some() {"Modifier la recherche"} else {"Votre prochaine voiture"}).open(&mut opened).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,Vec2::ZERO).default_width(440.0).show(ctx, |ui| {
            ui.add_space(10.0); ui.label(RichText::new("Nous trouvons les garages. Vous fixez les critères.").color(MUTED)); ui.add_space(20.0);
            ui.label("Marque"); ui.add(egui::TextEdit::singleline(&mut self.form_make).margin(Margin::symmetric(12,10)).hint_text("Renault, Peugeot, BMW…").desired_width(f32::INFINITY)); ui.add_space(12.0);
            ui.label("Modèle"); ui.add(egui::TextEdit::singleline(&mut self.form_model).margin(Margin::symmetric(12,10)).hint_text("Clio, 208, Série 3…").desired_width(f32::INFINITY)); ui.add_space(12.0);
            ui.label("Budget maximum"); ui.add(egui::DragValue::new(&mut self.form_budget).range(500..=2_000_000).speed(100).suffix(" €")); ui.add_space(16.0);
            vehicle_filters(ui, &mut self.form_filters);
            ui.add_space(12.0);
            ui.label(RichText::new("Recherche en France, sur les sites de professionnels. Le modèle et la marque doivent figurer dans le titre de l’annonce.").color(MUTED).size(12.0)); ui.add_space(20.0);
            let valid = !normalize(&self.form_make).is_empty() && !normalize(&self.form_model).is_empty() && self.form_budget>0 && self.form_filters.valid();
            ui.add_enabled_ui(valid, |ui| { if primary(ui,"Enregistrer et explorer").clicked() {submit=true;} }); ui.add_space(10.0);
        });
        if !opened {
            self.search_modal = false;
        }
        if submit {
            if let Some(id) = &self.edit_id {
                if let Some(search) = self.state.searches.iter_mut().find(|s| &s.id == id) {
                    search.make = self.form_make.trim().into();
                    search.model = self.form_model.trim().into();
                    search.max_price = self.form_budget;
                    search.filters = self.form_filters.clone();
                    search.enabled = true;
                }
            } else {
                self.state.searches.push(Search {
                    id: uuid::Uuid::new_v4().to_string(),
                    make: self.form_make.trim().into(),
                    model: self.form_model.trim().into(),
                    max_price: self.form_budget,
                    filters: self.form_filters.clone(),
                    enabled: true,
                    created_at: chrono::Utc::now().timestamp(),
                });
            }
            self.save();
            self.search_modal = false;
            self.page = Page::Overview;
            if self.scanning {
                self.scan_queued = true;
                self.cancel.store(true, Ordering::Relaxed);
                self.message(
                    "Recherche enregistrée. Nouvelle exploration après l’arrêt des pages en cours.",
                );
            } else {
                self.start_scan();
            }
        }
    }

    fn detail_dialog(&mut self, ctx: &egui::Context) {
        let Some(listing) = self
            .detail
            .as_ref()
            .and_then(|id| self.state.listings.iter().find(|l| &l.id == id))
            .cloned()
        else {
            return;
        };
        let mut open = true;
        egui::Window::new("Détail de l’annonce").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,Vec2::ZERO).default_width(510.0).show(ctx, |ui| {
            ui.add_space(12.0); ui.label(RichText::new(&listing.title).strong().size(24.0)); ui.add_space(12.0); ui.label(RichText::new(format!("{} €",number(listing.price))).size(32.0).strong().color(ACCENT)); ui.add_space(16.0);
            ui.label(format!("Garage : {}", listing.garage_name));
            if let Some(year)=listing.year {ui.label(format!("Année : {year}"));}
            if let Some(km)=listing.mileage {ui.label(format!("Kilométrage : {} km",number(km)));}
            if let Some(fuel)=&listing.fuel {ui.label(format!("Énergie : {fuel}"));}
            ui.add_space(10.0); ui.label(RichText::new(format!("Première détection : {} · Dernière observation : {}",date(listing.first_seen),date(listing.last_seen))).color(MUTED).size(11.0));
            ui.add_space(14.0); ui.label(RichText::new("La présence sur les grands portails n’est pas vérifiée. Contactez le garage pour confirmer le prix et la disponibilité.").color(MUTED)); ui.add_space(18.0);
            if primary(ui,"Voir sur le site du garage  ↗").clicked()
                && let Ok(url)=url::Url::parse(&listing.url)
                && ["https","http"].contains(&url.scheme())
                && let Err(e)=open::that(url.as_str()) {
                self.message(format!("Ouverture du navigateur impossible : {e}"));
            }
            ui.add_space(8.0);
        });
        if !open {
            self.detail = None;
        }
    }
}

impl eframe::App for GarageApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if self.installing && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.message("Installation de la mise à jour en cours. La fermeture sera possible dans quelques instants.");
        }
        self.sidebar(ctx);
        egui::TopBottomPanel::bottom("status")
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(26, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.scanning {
                        ui.spinner();
                        ui.label(RichText::new(&self.progress).color(MUTED).size(11.0));
                        if self.fraction > 0.0 {
                            ui.add(
                                egui::ProgressBar::new(self.fraction)
                                    .desired_width(120.0)
                                    .fill(ACCENT),
                            );
                        }
                    } else {
                        ui.label(
                            RichText::new(
                                self.state
                                    .last_scan
                                    .map(|t| format!("Dernière exploration {}", relative(t)))
                                    .unwrap_or_else(|| "Prêt à explorer les garages".into()),
                            )
                            .size(11.0)
                            .color(MUTED),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(if self.demo {
                                "DÉMONSTRATION · DONNÉES FICTIVES"
                            } else {
                                "FRANCE  /  EUR  /  DIRECT GARAGES"
                            })
                            .size(9.0)
                            .color(if self.demo { AMBER } else { MUTED })
                            .extra_letter_spacing(1.0),
                        );
                    });
                });
            });
        egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(30,28))).show(ctx, |ui| {
            if let Some(error)=&self.storage_error {ui.label(RichText::new(error).color(AMBER));ui.add_space(10.0);}
            if let Some((message,_))=&self.toast {Frame::new().fill(Color32::from_rgb(37,47,36)).corner_radius(8).inner_margin(12).show(ui,|ui|{ui.set_width(ui.available_width());ui.label(RichText::new(message).color(ACCENT));});ui.add_space(12.0);}
            egui::ScrollArea::vertical().auto_shrink([false,false]).show(ui, |ui| {
                match self.page {
                    Page::Overview => self.overview(ui),
                    Page::Listings => {self.header(ui,"Les annonces", "Votre tableau de bord des occasions repérées chez les professionnels.");self.listing_table(ui,false);}
                    Page::Searches => self.searches(ui), Page::Garages => self.garages(ui), Page::Alerts => self.alerts(ui), Page::Settings => self.settings(ui),
                }
            });
        });
        self.search_dialog(ctx);
        self.detail_dialog(ctx);
        if let Some(path) = &self.screenshot {
            if !self.screenshot_requested && self.started.elapsed() > Duration::from_secs(2) {
                self.screenshot_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            for event in ctx.input(|i| i.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = event {
                    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                    if let Err(e) = image::save_buffer(
                        path,
                        &bytes,
                        image.size[0] as u32,
                        image.size[1] as u32,
                        image::ColorType::Rgba8,
                    ) {
                        eprintln!("Screenshot: {e}");
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }
}

fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals(egui::Visuals::dark());
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = Color32::from_rgb(24, 30, 38);
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.selection.bg_fill = Color32::from_rgb(60, 78, 43);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.window_corner_radius = 14.into();
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(30, 37, 47);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(30, 37, 47);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(45, 56, 43);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(45, 56, 43);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(16.0, 10.0);
    style.spacing.interact_size = Vec2::new(40.0, 34.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(13.0));
    ctx.set_style(style);
}
fn card() -> Frame {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(12)
        .inner_margin(18)
}

fn vehicle_filters(ui: &mut egui::Ui, filters: &mut VehicleFilters) {
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut filters.electric_only, "Électrique uniquement")
            .on_hover_text("Véhicules 100 % électriques ; hybrides exclus.");
        if *filters != VehicleFilters::default() && ui.small_button("Réinitialiser").clicked() {
            *filters = VehicleFilters::default();
        }
    });
    let this_year = chrono::Datelike::year(&chrono::Utc::now()) as u16;
    ui.horizontal_wrapped(|ui| {
        year_filter(ui, "Année min.", &mut filters.min_year, 2018, this_year + 1);
        year_filter(
            ui,
            "Année max.",
            &mut filters.max_year,
            this_year,
            this_year + 1,
        );
    });
    if !filters.valid() {
        ui.label(
            RichText::new("L’année minimum doit précéder ou égaler l’année maximum.")
                .color(AMBER)
                .size(12.0),
        );
    } else if *filters != VehicleFilters::default() {
        ui.label(
            RichText::new("Les annonces sans l’énergie ou l’année demandée sont exclues.")
                .color(MUTED)
                .size(11.0),
        );
    }
}

fn year_filter(ui: &mut egui::Ui, label: &str, year: &mut Option<u16>, default: u16, max: u16) {
    ui.horizontal(|ui| {
        let mut enabled = year.is_some();
        ui.checkbox(&mut enabled, label)
            .on_hover_text("Borne incluse. Désactiver pour ne pas limiter l’année.");
        let mut value = year.unwrap_or(default);
        ui.add_enabled(
            enabled,
            egui::DragValue::new(&mut value).range(1900..=max).speed(1),
        );
        *year = enabled.then_some(value);
    });
}

fn nav_button(ui: &mut egui::Ui, label: &str, page: Page, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(187.0, 44.0), egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::SelectableLabel, true, label));
    let painter = ui.painter();
    if selected || response.hovered() {
        painter.rect_filled(
            rect,
            8,
            if selected {
                Color32::from_rgb(36, 47, 35)
            } else {
                PANEL
            },
        );
    }
    let color = if selected { ACCENT } else { MUTED };
    let center = egui::pos2(rect.left() + 17.0, rect.center().y);
    let stroke = Stroke::new(1.5_f32, color);
    let point = |x, y| center + Vec2::new(x, y);
    match page {
        Page::Overview => {
            for (x, y) in [(-4.0, -4.0), (4.0, -4.0), (-4.0, 4.0), (4.0, 4.0)] {
                painter.rect_stroke(
                    egui::Rect::from_center_size(point(x, y), Vec2::splat(5.0)),
                    1,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Page::Listings => {
            for y in [-5.0, 0.0, 5.0] {
                painter.line_segment([point(-6.0, y), point(6.0, y)], stroke);
            }
        }
        Page::Searches => {
            painter.circle_stroke(point(-2.0, -2.0), 5.0, stroke);
            painter.line_segment([point(2.0, 2.0), point(7.0, 7.0)], stroke);
        }
        Page::Garages => {
            painter.add(egui::Shape::line(
                vec![point(-7.0, -1.0), point(0.0, -7.0), point(7.0, -1.0)],
                stroke,
            ));
            painter.rect_stroke(
                egui::Rect::from_min_max(point(-5.0, -1.0), point(5.0, 7.0)),
                0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([point(0.0, 2.0), point(0.0, 7.0)], stroke);
        }
        _ => {
            painter.circle_stroke(center, 6.0, stroke);
            painter.circle_filled(center, 2.0, color);
        }
    }
    painter.text(
        egui::pos2(rect.left() + 37.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        color,
    );
    response
}
fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(BG).strong().size(13.0))
            .fill(ACCENT)
            .stroke(Stroke::NONE)
            .corner_radius(8)
            .min_size(Vec2::new(150.0, 42.0)),
    )
}
fn secondary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0))
            .corner_radius(8)
            .min_size(Vec2::new(90.0, 36.0)),
    )
}
fn number(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}
fn relative(time: i64) -> String {
    let delta = (chrono::Utc::now().timestamp() - time).max(0);
    if delta < 60 {
        "à l’instant".into()
    } else if delta < 3600 {
        format!("il y a {} min", delta / 60)
    } else if delta < 86400 {
        format!("il y a {} h", delta / 3600)
    } else {
        format!("il y a {} j", delta / 86400)
    }
}
fn date(time: i64) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d/%m/%Y %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}

/// Explicit demo mode never reads/writes user data and never performs network requests.
fn demo_state() -> State {
    let now = chrono::Utc::now().timestamp();
    let mut state = State {
        searches: vec![
            Search {
                id: "demo-clio".into(),
                make: "Renault".into(),
                model: "Clio".into(),
                max_price: 15000,
                filters: Default::default(),
                enabled: true,
                created_at: now,
            },
            Search {
                id: "demo-208".into(),
                make: "Peugeot".into(),
                model: "208".into(),
                max_price: 16000,
                filters: Default::default(),
                enabled: true,
                created_at: now,
            },
            Search {
                id: "demo-golf".into(),
                make: "Volkswagen".into(),
                model: "Golf".into(),
                max_price: 18000,
                filters: Default::default(),
                enabled: true,
                created_at: now,
            },
        ],
        last_scan: Some(now - 240),
        ..State::default()
    };
    for (i, (title, garage, price, km, year, fuel)) in [
        (
            "Renault Clio V Intens TCe 90",
            "Garage des Tilleuls",
            12990,
            38500,
            2021,
            "Essence",
        ),
        (
            "Peugeot 208 e-208 Allure 136 ch",
            "Automobiles du Parc",
            14490,
            27800,
            2022,
            "Électrique",
        ),
        (
            "Volkswagen Golf VII 1.4 TSI",
            "Garage de la Vallée",
            16700,
            56200,
            2019,
            "Essence",
        ),
        (
            "Renault Clio V Business Blue dCi",
            "Atelier Saint-Martin",
            11900,
            67400,
            2020,
            "Diesel",
        ),
        (
            "Peugeot 208 GT Line PureTech",
            "Garage des Tilleuls",
            15300,
            31200,
            2021,
            "Essence",
        ),
    ]
    .iter()
    .enumerate()
    {
        let id = format!("demo-{i}");
        state.garages.push(Garage {
            id: id.clone(),
            name: garage.to_string(),
            url: format!("https://garage-{i}.example"),
            enabled: true,
            selectors: Selectors::default(),
            last_scan: Some(now - 240),
            last_error: None,
        });
        state.merge_listings(vec![Listing {
            id: id.clone(),
            garage_id: id,
            garage_name: garage.to_string(),
            title: title.to_string(),
            price: *price,
            url: format!("https://garage-{i}.example/occasion"),
            mileage: Some(*km),
            year: Some(*year),
            fuel: Some(fuel.to_string()),
            first_seen: now - ((i as i64 + 1) * 900),
            last_seen: now - 240,
            favorite: i == 0,
        }]);
    }
    state
}
