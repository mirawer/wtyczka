//! Multiple SNI objects on one connection, with distinct paths for Noctalia.
//! Separate ksni services all use /StatusNotifierItem, which Noctalia deduplicates by PID+path.
use super::tray::Indicator;
use crate::{display::State, mode::AnswerMode};
use anyhow::Result;
use futures_util::StreamExt;
use ksni::Tray;
use zbus::{Connection, Proxy, object_server::SignalEmitter};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
type Tooltip = (String, Pixmaps, String, String);

struct Item(Indicator);

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> String {
        self.0.id()
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        if self.0.visible { "Active" } else { "Passive" }
    }
    #[zbus(property)]
    fn window_id(&self) -> u32 {
        0
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        self.0
            .icon_pixmap()
            .into_iter()
            .map(|i| (i.width, i.height, i.data))
            .collect()
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }
    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn tool_tip(&self) -> Tooltip {
        (String::new(), Vec::new(), String::new(), String::new())
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> zbus::zvariant::ObjectPath<'static> {
        zbus::zvariant::ObjectPath::from_static_str("/NO_DBUSMENU").expect("constant valid path")
    }
    fn activate(&self, _x: i32, _y: i32) {}
    fn secondary_activate(&self, _x: i32, _y: i32) {}
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
}

pub struct Cells {
    connection: Connection,
    paths: Vec<String>,
    watcher: tokio::task::JoinHandle<()>,
}

async fn register(watcher: &Proxy<'_>, paths: &[String]) -> zbus::Result<()> {
    for path in paths {
        watcher
            .call::<_, _, ()>("RegisterStatusNotifierItem", &(path.as_str(),))
            .await?;
    }
    Ok(())
}

impl Cells {
    pub async fn spawn(mode: AnswerMode, count: usize) -> Result<Self> {
        let connection = zbus::connection::Builder::session()?
            .name(format!(
                "org.kde.StatusNotifierItem-{}-1",
                std::process::id()
            ))?
            .build()
            .await?;
        let paths: Vec<_> = (0..count)
            .map(|slot| format!("/ScreenAnswer/{slot:02}"))
            .collect();
        for (slot, path) in paths.iter().enumerate() {
            connection
                .object_server()
                .at(
                    path.as_str(),
                    Item(Indicator {
                        state: State::Idle,
                        mode,
                        slot,
                        visible: slot == 0,
                    }),
                )
                .await?;
        }
        let proxy = Proxy::new_owned(connection.clone(), WATCHER, WATCHER_PATH, WATCHER).await?;
        let mut owners = proxy.receive_owner_changed().await?;
        register(&proxy, &paths).await?;
        let registered_paths = paths.clone();
        let watcher = tokio::spawn(async move {
            while let Some(owner) = owners.next().await {
                if owner.is_some() {
                    // A shell restart must not permanently lose the answer indicator.
                    for attempt in 0..3 {
                        if register(&proxy, &registered_paths).await.is_ok() {
                            break;
                        }
                        if attempt == 2 {
                            eprintln!(
                                "Nie udało się ponownie zarejestrować wskaźnika w zasobniku."
                            );
                        } else {
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        }
                    }
                }
            }
        });
        Ok(Self {
            connection,
            paths,
            watcher,
        })
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub async fn update(&self, slot: usize, state: State, visible: bool) -> Result<()> {
        let interface = self
            .connection
            .object_server()
            .interface::<_, Item>(self.paths[slot].as_str())
            .await?;
        let status = if visible { "Active" } else { "Passive" };
        {
            let mut item = interface.get_mut().await;
            item.0.state = state;
            item.0.visible = visible;
        }
        Item::new_icon(interface.signal_emitter()).await?;
        Item::new_status(interface.signal_emitter(), status).await?;
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.watcher.abort();
        let _ = self.connection.clone().close().await;
    }
}

impl Drop for Cells {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}
