//! A window narrower than the page (0.16.1): the web build opens in Web Layout, as Word does in a
//! narrow window, so no line of the document is cut off at the window's right edge. The page's
//! address can ask for a view instead (`?view=web`, `?zoom=pageWidth`). Only the web build turns
//! this on ([`AutoView`] is made by its shell); the desktop app opens as before.

use super::{GAP, PX_PER_PT, RULER, SCROLLBAR};
use crate::WordApp;
use serde_json::json;
use wordcraft_layout::ViewMode;

/// What the page's address asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewAsk {
    /// Nothing: choose by the window's width ([`AutoView`]).
    #[default]
    Auto,
    /// `?view=web`: Web Layout.
    Web,
    /// `?zoom=pageWidth`: Print Layout, the page fitted to the window's width.
    PageWidth,
}

/// Read `?view=web` and `?zoom=pageWidth` from an address's query (with or without its `?`).
/// `view=web` wins when both are given.
pub fn view_ask(query: &str) -> ViewAsk {
    let mut ask = ViewAsk::Auto;
    for pair in query.trim_start_matches('?').split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match (key, value) {
            ("view", "web") => return ViewAsk::Web,
            ("zoom", "pageWidth") => ask = ViewAsk::PageWidth,
            _ => {}
        }
    }
    ask
}

/// The width (screen points) a page `page_w_pt` wide needs in Print Layout at 100 percent: the
/// page, the canvas's margin on each side as Page Width leaves it, the scroll bar, and the
/// vertical ruler when it is shown.
pub fn print_page_needs(page_w_pt: f32, ruler: bool) -> f32 {
    page_w_pt * PX_PER_PT + 2.0 * GAP + 20.0 + SCROLLBAR + if ruler { RULER } else { 0.0 }
}

/// The view the person sees, as far as choosing one goes.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    mode: ViewMode,
    read: bool,
    fit: String,
    zoom: f32,
}

impl Shown {
    fn of(app: &WordApp) -> Self {
        let v = &app.session.view;
        // A fit mode's zoom follows the window (sync_fit_zoom), so only a plain zoom counts.
        let zoom = if v.fit.is_empty() { (v.zoom * 100.0).round() } else { 0.0 };
        Shown { mode: v.mode, read: v.read_mode, fit: v.fit.clone(), zoom }
    }
}

/// The web build's view choice. Until the person picks a view or a zoom, it follows the work
/// area's width: Web Layout while the page at 100 percent does not fit, Print Layout once it does.
/// The view it set is set again when the host opens a document (opening restores the file's
/// saved zoom, which would undo `?zoom=pageWidth`). Once the person changes the view or the zoom
/// themselves, it never changes the view again.
#[derive(Debug)]
pub struct AutoView {
    ask: ViewAsk,
    /// What it set last, or `None` once the person chose their own view.
    own: Option<Shown>,
    /// The document it set the view for.
    doc: Option<u64>,
}

impl AutoView {
    pub fn new(ask: ViewAsk) -> Self {
        AutoView { ask, own: None, doc: None }
    }

    /// Whether it still chooses the view (the person has not chosen one).
    pub fn choosing(&self) -> bool {
        self.doc.is_none() || self.own.is_some()
    }

    /// Run once per frame, before the app draws, with the work area's width in screen points
    /// (the canvas's, or the window's before the canvas is drawn).
    pub fn apply(&mut self, app: &mut WordApp, work_w: f32) {
        let doc = app.session.document_id();
        let first = self.doc.is_none();
        if !first && self.own.is_none() {
            return;
        }
        let opened = self.doc.is_some_and(|d| d != doc);
        if !first && !opened && self.own.as_ref() != Some(&Shown::of(app)) {
            // The person changed the view or the zoom: theirs from now on.
            self.own = None;
            return;
        }
        self.doc = Some(doc);
        match self.ask {
            ViewAsk::Web => self.set_web(app),
            ViewAsk::PageWidth => {
                let _ = app.run("view.printLayout", json!({}));
                let _ = app.run("view.zoom", json!({"value": "pageWidth"}));
            }
            ViewAsk::Auto => {
                let page_w = app.session.doc.sections().first().map_or(612.0, |(_, s)| s.page_w);
                let ruler = app.session.view.ruler;
                if work_w.is_finite() && work_w > 0.0 && work_w < print_page_needs(page_w, ruler) {
                    self.set_web(app);
                } else if app.session.view.mode == ViewMode::Web && !app.session.view.read_mode {
                    let _ = app.run("view.printLayout", json!({}));
                }
            }
        }
        self.own = Some(Shown::of(app));
    }

    fn set_web(&self, app: &mut WordApp) {
        if app.session.view.mode != ViewMode::Web || app.session.view.read_mode {
            let _ = app.run("view.webLayout", json!({}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> WordApp {
        WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::sample::sample_document()), crate::Services::default())
    }

    /// The app as the web build runs it: its own frame, with or without the view choice.
    struct Shell {
        app: WordApp,
        auto: Option<AutoView>,
    }

    /// Lines of the body whose right edge passes `right` (screen points), on the pages drawn last
    /// frame, each named by its page and its top.
    fn cut_lines(app: &WordApp, right: f32) -> Vec<String> {
        let l = app.session.layout();
        let s = app.canvas.scale;
        let mut cut = Vec::new();
        for (i, page) in l.pages.iter().enumerate() {
            let Some(r) = app.canvas.page_rects.get(i) else { continue };
            for line in wordcraft_layout::hit::page_lines(page, wordcraft_doc::StoryRef::Body) {
                let x1 = r.min.x + line.right * s;
                if x1 > right + 0.5 {
                    cut.push(format!("page {} line at {:.0}pt ends at {x1:.0}px", i + 1, line.top));
                }
            }
        }
        cut
    }

    /// Runs the whole app `w` by `h` with the sample document (the web build's `?sample`), the
    /// view choice made as the web build makes it, or none (`auto` false: 0.16.0's Print Layout at
    /// 100 percent).
    fn run_app(w: f32, h: f32, auto: bool) -> egui_kittest::Harness<'static, Shell> {
        let shell = Shell { app: app(), auto: auto.then(|| AutoView::new(ViewAsk::Auto)) };
        let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(w, h)).build_ui_state(
            |ui, s: &mut Shell| {
                let ctx = ui.ctx().clone();
                s.app.logic(&ctx);
                if let Some(auto) = &mut s.auto {
                    let work = s.app.canvas.canvas_rect.map_or(ctx.content_rect().width(), |r| r.width());
                    auto.apply(&mut s.app, work);
                }
                s.app.ui(ui);
            },
            shell,
        );
        harness.run_steps(8);
        harness
    }

    /// Social Media's Round 6 reply in Prepara's frame: WordCraft in a 482 px column. Every line
    /// of the document stays inside the window; 0.16.0's Print Layout at 100 percent (planted) cut
    /// every line at the right.
    #[test]
    fn at_482_no_line_is_cut_at_the_right() {
        let h = run_app(482.0, 820.0, true);
        let s = h.state();
        assert_eq!(s.app.session.view.mode, ViewMode::Web);
        assert!(!s.app.canvas.page_rects.is_empty(), "the document is drawn");
        let cut = cut_lines(&s.app, 482.0);
        assert!(cut.is_empty(), "lines cut at the right: {cut:?}");
        // PLANTED: the same window in Print Layout at 100 percent, as 0.16.0 opened it.
        let planted = run_app(482.0, 820.0, false);
        assert_eq!(planted.state().app.session.view.mode, ViewMode::Print);
        assert!(!cut_lines(&planted.state().app, 482.0).is_empty(), "the check refuses Print Layout at 100 percent in 482");
        // A wide window keeps Print Layout and its lines inside.
        let wide = run_app(1200.0, 820.0, true);
        assert_eq!(wide.state().app.session.view.mode, ViewMode::Print);
        assert!(cut_lines(&wide.state().app, 1200.0).is_empty());
    }

    #[test]
    fn the_address_asks_for_a_view() {
        assert_eq!(view_ask(""), ViewAsk::Auto);
        assert_eq!(view_ask("?host=parent&view=web"), ViewAsk::Web);
        assert_eq!(view_ask("zoom=pageWidth&author=A"), ViewAsk::PageWidth);
        assert_eq!(view_ask("?zoom=pageWidth&view=web"), ViewAsk::Web);
        assert_eq!(view_ask("?view=print&zoom=100"), ViewAsk::Auto);
    }

    #[test]
    fn a_narrow_window_opens_in_web_layout_and_a_wide_one_in_print_layout() {
        let mut a = app();
        let mut auto = AutoView::new(ViewAsk::Auto);
        auto.apply(&mut a, 482.0);
        assert_eq!(a.session.view.mode, ViewMode::Web, "482 is narrower than a page at 100 percent");
        let mut b = app();
        let mut auto = AutoView::new(ViewAsk::Auto);
        auto.apply(&mut b, 1200.0);
        assert_eq!(b.session.view.mode, ViewMode::Print, "1200 holds the page");
        // The window that settles wider after load follows it, while the view is still its own.
        let mut c = app();
        let mut auto = AutoView::new(ViewAsk::Auto);
        auto.apply(&mut c, 482.0);
        auto.apply(&mut c, 1200.0);
        assert_eq!(c.session.view.mode, ViewMode::Print);
        auto.apply(&mut c, 482.0);
        assert_eq!(c.session.view.mode, ViewMode::Web);
    }

    #[test]
    fn the_persons_own_view_is_never_overridden() {
        let mut a = app();
        let mut auto = AutoView::new(ViewAsk::Auto);
        auto.apply(&mut a, 482.0);
        a.run("view.printLayout", json!({})).unwrap();
        auto.apply(&mut a, 482.0);
        assert_eq!(a.session.view.mode, ViewMode::Print, "the person chose Print Layout");
        assert!(!auto.choosing());
        // A zoom of their own counts too.
        let mut b = app();
        let mut auto = AutoView::new(ViewAsk::Auto);
        auto.apply(&mut b, 1200.0);
        b.run("view.zoom", json!({"value": 150})).unwrap();
        auto.apply(&mut b, 482.0);
        assert_eq!(b.session.view.mode, ViewMode::Print, "the person zoomed: no Web Layout after");
    }

    #[test]
    fn an_opened_document_keeps_the_chosen_view() {
        let mut a = app();
        let mut auto = AutoView::new(ViewAsk::Auto);
        auto.apply(&mut a, 482.0);
        // As the host's wordcraft:open does (app/files.rs drain_inbox): file.open with the bytes of a
        // .docx saved at 120 percent, which restores that zoom.
        let mut doc = wordcraft_engine::sample::sample_document();
        doc.settings.zoom = Some(wordcraft_doc::Zoom { percent: 120, fit: None });
        let bytes = wordcraft_engine::io::save_bytes("task.docx", &doc).expect("a .docx");
        let open = |a: &mut WordApp| {
            a.run("file.open", json!({"path": "task.docx", "data": wordcraft_engine::cmd::insert::base64_encode(&bytes)})).expect("opens");
        };
        open(&mut a);
        auto.apply(&mut a, 482.0);
        assert_eq!(a.session.view.mode, ViewMode::Web, "opening a document does not reset the view");
        assert!(auto.choosing(), "an open is not the person's own choice");
        // ?zoom=pageWidth is set again after an open that restored a saved zoom.
        let mut b = app();
        let mut auto = AutoView::new(ViewAsk::PageWidth);
        auto.apply(&mut b, 482.0);
        assert_eq!((b.session.view.mode, b.session.view.fit.as_str()), (ViewMode::Print, "pageWidth"));
        open(&mut b);
        assert_eq!(b.session.view.fit, "", "the open restored the file's 120 percent");
        auto.apply(&mut b, 482.0);
        assert_eq!(b.session.view.fit, "pageWidth");
        // ?view=web holds at any width.
        let mut c = app();
        let mut auto = AutoView::new(ViewAsk::Web);
        auto.apply(&mut c, 1400.0);
        assert_eq!(c.session.view.mode, ViewMode::Web);
    }
}
