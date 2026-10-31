//! The Print portal (`org.freedesktop.impl.portal.Print`): another
//! program's Print… (GTK's, a sandboxed app's) shows Olo's Print panel
//! (`olo_print::panel`). PreparePrint answers with the printer, copies,
//! pages and paper (as GTK's print settings and page setup) and a token;
//! the program then lays its pages out for that paper and hands Print the
//! document (a PDF, or PostScript), which goes to the printer through CUPS
//! (`olo_print::ipp`), or, for "PDF File", where the person says.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::AtomicU32},
};

use olo_print::panel::Choice;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::portal::ToUi;

pub struct Print {
    pub to_ui: Arc<dyn Fn(ToUi) + Send + Sync>,
    /// What PreparePrint chose, by the token it gave.
    pub tokens: Arc<Mutex<HashMap<u32, Choice>>>,
    pub next: AtomicU32,
}

/// The Print panel to show for a request.
pub struct Ask {
    pub request: String,
    pub parent: String,
    pub title: String,
    /// The choice, or None if cancelled.
    pub reply: async_channel::Sender<Option<Choice>>,
}

fn s(v: impl Into<String>) -> Value<'static> {
    Value::from(v.into())
}

/// The choice as GTK's print settings (strings, as GtkPrintSettings
/// keeps them).
pub fn settings(c: &Choice) -> HashMap<String, Value<'static>> {
    let mut m = HashMap::from([("n-copies".to_owned(), s(c.copies.to_string()))]);
    match &c.printer {
        Some(p) => {
            m.insert("printer".into(), s(p.clone()));
        }
        None => {
            m.insert("output-file-format".into(), s("pdf"));
        }
    }
    if let Some((a, b)) = c.pages {
        // GTK counts pages from 0.
        m.insert("print-pages".into(), s("ranges"));
        m.insert("page-ranges".into(), s(format!("{}-{}", a.saturating_sub(1), b.saturating_sub(1))));
    }
    m
}

/// The choice's paper as GTK's page setup (millimetres).
pub fn page_setup(c: &Choice) -> HashMap<String, Value<'static>> {
    let mm = |pt: f32| f64::from(pt) * 25.4 / 72.0;
    let name = c.paper.pwg.split('_').take(2).collect::<Vec<_>>().join("_");
    HashMap::from([
        ("PPDName".to_owned(), s(c.paper.pwg)),
        ("Name".to_owned(), s(name)),
        ("DisplayName".to_owned(), s(c.paper.label)),
        ("Width".to_owned(), Value::from(mm(c.paper.width))),
        ("Height".to_owned(), Value::from(mm(c.paper.height))),
        ("MarginTop".to_owned(), Value::from(6.35f64)),
        ("MarginBottom".to_owned(), Value::from(6.35f64)),
        ("MarginLeft".to_owned(), Value::from(6.35f64)),
        ("MarginRight".to_owned(), Value::from(6.35f64)),
        ("Orientation".to_owned(), s("portrait")),
    ])
}

fn owned(v: Value<'static>) -> OwnedValue {
    OwnedValue::try_from(v).expect("no file descriptors")
}

impl Print {
    /// Show the Print panel for request `handle`; the choice.
    async fn ask(&self, handle: &OwnedObjectPath, parent: String, title: String, server: &zbus::ObjectServer) -> Option<Choice> {
        let (tx, rx) = async_channel::bounded(1);
        let request = crate::portal::RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
        let _ = server.at(handle.as_ref(), request).await;
        (self.to_ui)(ToUi::Print(Ask { request: handle.to_string(), parent, title, reply: tx }));
        let chosen = rx.recv().await.ok().flatten();
        let _ = server.remove::<crate::portal::RequestObject, _>(handle.as_ref()).await;
        chosen
    }

    /// Where to save a PDF (Olo's Save panel, through the FileChooser's
    /// way of asking).
    async fn save_where(&self, handle: &OwnedObjectPath, parent: String, name: String) -> Option<std::path::PathBuf> {
        let (tx, rx) = async_channel::bounded(1);
        let folder = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Documents")).filter(|d| d.is_dir());
        (self.to_ui)(ToUi::Files(crate::files::Ask {
            request: handle.to_string(),
            parent,
            title: "Save as PDF".into(),
            purpose: crate::files::Purpose::Save,
            folder,
            name,
            filters: Vec::new(),
            current: 0,
            multiple: false,
            choices: Vec::new(),
            reply: tx,
        }));
        rx.recv().await.ok().flatten().and_then(|c| c.paths.into_iter().next())
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Print")]
impl Print {
    #[allow(clippy::too_many_arguments)]
    async fn prepare_print(
        &self,
        handle: OwnedObjectPath,
        _app_id: String,
        parent_window: String,
        title: String,
        _settings: HashMap<String, OwnedValue>,
        _page_setup: HashMap<String, OwnedValue>,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let Some(choice) = self.ask(&handle, parent_window, title, server).await else { return (1, HashMap::new()) };
        let token = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let results = HashMap::from([
            ("settings".to_owned(), owned(Value::from(settings(&choice)))),
            ("page-setup".to_owned(), owned(Value::from(page_setup(&choice)))),
            ("token".to_owned(), owned(Value::from(token))),
        ]);
        self.tokens.lock().unwrap_or_else(|e| e.into_inner()).insert(token, choice);
        (0, results)
    }

    #[allow(clippy::too_many_arguments)]
    async fn print(
        &self,
        handle: OwnedObjectPath,
        _app_id: String,
        parent_window: String,
        title: String,
        fd: zbus::zvariant::OwnedFd,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        // Prepared already (a token), else asked now.
        let token = options.get("token").and_then(|v| u32::try_from(v).ok());
        let prepared = token.and_then(|t| self.tokens.lock().unwrap_or_else(|e| e.into_inner()).remove(&t));
        let choice = match prepared {
            Some(c) => c,
            None => match self.ask(&handle, parent_window.clone(), title.clone(), server).await {
                Some(c) => c,
                None => return (1, HashMap::new()),
            },
        };
        let fd: std::os::fd::OwnedFd = fd.into();
        let document = blocking::unblock(move || {
            use std::io::Read;
            let mut data = Vec::new();
            std::fs::File::from(fd).read_to_end(&mut data).map(|_| data)
        })
        .await;
        let Ok(document) = document else { return (2, HashMap::new()) };
        let postscript = document.starts_with(b"%!PS");
        let name = if title.is_empty() { "Document".to_owned() } else { title.clone() };
        let result = match choice.printer.clone() {
            Some(printer) => {
                let job = olo_print::ipp::Job { name, copies: choice.copies, media: Some(choice.paper.pwg.into()), pages: None, two_sided: false };
                let format = if postscript { "application/postscript" } else { "application/pdf" };
                blocking::unblock(move || olo_print::ipp::print_document(&printer, &document, format, &job).map(|_| ())).await
            }
            None => {
                let ext = if postscript { "ps" } else { "pdf" };
                match self.save_where(&handle, parent_window, format!("{name}.{ext}")).await {
                    Some(path) => blocking::unblock(move || std::fs::write(path, document).map_err(|e| e.to_string())).await,
                    None => return (1, HashMap::new()),
                }
            }
        };
        match result {
            Ok(()) => (0, HashMap::new()),
            Err(e) => {
                tracing::warn!("portal: couldn't print “{title}”: {e}");
                (2, HashMap::new())
            }
        }
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtk_settings_and_page_setup() {
        let c = Choice { printer: Some("Office".into()), copies: 2, pages: Some((2, 4)), paper: olo_print::PAPERS[2] };
        let st = settings(&c);
        assert_eq!(st.get("printer"), Some(&Value::from("Office")));
        assert_eq!(st.get("n-copies"), Some(&Value::from("2")));
        assert_eq!(st.get("page-ranges"), Some(&Value::from("1-3")));
        let ps = page_setup(&c);
        assert_eq!(ps.get("Name"), Some(&Value::from("iso_a4")));
        let Some(Value::F64(w)) = ps.get("Width") else { panic!() };
        assert!((w - 210.0).abs() < 0.1, "{w}");
        let pdf = Choice { printer: None, ..c };
        assert_eq!(settings(&pdf).get("output-file-format"), Some(&Value::from("pdf")));
    }
}
