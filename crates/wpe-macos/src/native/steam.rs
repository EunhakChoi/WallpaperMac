use super::*;
use wpe_platform::steam::{ItemUpdate, SteamConfig, SteamProvider};
impl AppDelegate {
    pub(super) fn restart_steam(&self, explicit: bool) {
        let config = self.ivars().borrow().service.config.steam.clone();
        {
            let mut state = self.ivars().borrow_mut();
            if !explicit && state.steam_config == config {
                return;
            }
            state.steam.take();
            state.steam_config = config.clone();
            state.steam_error = None;
        }
        if let Some(config) = config {
            match SteamProvider::load(&config) {
                Ok(provider) => self.ivars().borrow_mut().steam = Some(provider),
                Err(error) => {
                    self.ivars().borrow_mut().steam_error = Some(error.to_string());
                    self.record_error(&error.to_string());
                }
            }
        }
    }
    pub(super) fn steam_status(&self) -> Value {
        let state = self.ivars().borrow();
        json!({"configured":state.steam_config,"connected":state.steam.is_some(),"error":state.steam_error,"progress":state.steam.as_ref().map(SteamProvider::pending_progress),"achievements":state.steam.as_ref().map(SteamProvider::achievement_status),"events":state.steam_events,"abi_profile":"SteamUGC_v020 / Utils_v010; official Mac SDK header probe not run on this host"})
    }
    pub(super) fn steam_command(&self, method: &str, args: Value) -> Result<Value, String> {
        if method == "steam.status" {
            return Ok(self.steam_status());
        }
        if method == "steam.connect" {
            if args.get("library").is_some() {
                let config: SteamConfig =
                    serde_json::from_value(args).map_err(|e| e.to_string())?;
                let mut c = self.ivars().borrow().service.config.clone();
                c.steam = Some(config);
                let reply = self
                    .ivars()
                    .borrow_mut()
                    .service
                    .execute("config.set", json!({"config":c}))
                    .map_err(|e| e.to_string())?;
                self.actions(reply.actions)?;
            }
            self.restart_steam(true);
            let status = self.steam_status();
            if let Some(error) = status["error"].as_str() {
                return Err(error.into());
            }
            return Ok(status);
        }
        let id = || {
            args.get("id")
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .filter(|v| *v > 0)
                .ok_or_else(|| "A positive Steam item ID is required".to_string())
        };
        let mut state = self.ivars().borrow_mut();
        let provider = state
            .steam
            .as_mut()
            .ok_or("Configure and connect an official Steamworks SDK library first")?;
        match method {
            "steam.achievement.publish" => provider
                .unlock_publish_achievement(args["kind"].as_str().ok_or("Wallpaper kind required")?)
                .map_err(|e| e.to_string()),
            "steam.dlcs" => Ok(json!(provider.dlcs().map_err(|e| e.to_string())?)),
            "steam.dlc.status" | "steam.dlc.install" | "steam.dlc.uninstall" => {
                let id: u32 = id()?.try_into().map_err(|_| "DLC AppID exceeds32bits")?;
                let info = if method == "steam.dlc.status" {
                    provider.dlc(id)
                } else {
                    provider.install_dlc(id, method == "steam.dlc.install")
                }
                .map_err(|e| e.to_string())?;
                Ok(json!({"requested":method!="steam.dlc.status","status":info}))
            }
            "steam.published" => Ok(
                json!({"pending":true,"operation":provider.query_published(args["page"].as_u64().unwrap_or(1).try_into().map_err(|_|"Page overflow")?).map_err(|e|e.to_string())?.to_string()}),
            ),
            "steam.delete" => Ok(
                json!({"pending":true,"operation":provider.delete_item(id()?).map_err(|e|e.to_string())?.to_string()}),
            ),
            "steam.subscriptions" => {
                let ids = provider.subscribed().map_err(|e| e.to_string())?;
                Ok(
                    json!({"items":ids.iter().map(|id|json!({"id":id,"install":provider.install_info(*id).ok().flatten()})).collect::<Vec<_>>()}),
                )
            }
            "steam.query" => {
                let call = if let Some(ids) = args["ids"].as_array() {
                    let ids = ids
                        .iter()
                        .map(|v| {
                            v.as_u64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                                .ok_or("Invalid item ID")
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    provider.query_details(&ids)
                } else {
                    provider.query_filtered(
                        args["page"]
                            .as_u64()
                            .unwrap_or(1)
                            .try_into()
                            .map_err(|_| "Page overflow")?,
                        args["query_type"]
                            .as_i64()
                            .unwrap_or(0)
                            .try_into()
                            .map_err(|_| "Query type overflow")?,
                        args["matching_type"]
                            .as_i64()
                            .unwrap_or(0)
                            .try_into()
                            .map_err(|_| "Matching type overflow")?,
                        args["text"].as_str(),
                        &string_array(&args["required_tags"])?,
                        &string_array(&args["excluded_tags"])?,
                    )
                }
                .map_err(|e| e.to_string())?;
                Ok(json!({"pending":true,"operation":call.to_string()}))
            }
            "steam.subscribe" | "steam.unsubscribe" => {
                let call = provider
                    .subscribe(id()?, method == "steam.subscribe")
                    .map_err(|e| e.to_string())?;
                Ok(json!({"pending":true,"operation":call.to_string()}))
            }
            "steam.download" => {
                let id = id()?;
                provider
                    .download(id, args["high_priority"].as_bool().unwrap_or(true))
                    .map_err(|e| e.to_string())?;
                Ok(json!({"pending":true,"item":id.to_string()}))
            }
            "steam.installInfo" => Ok(json!(
                provider.install_info(id()?).map_err(|e| e.to_string())?
            )),
            "steam.create" => Ok(
                json!({"pending":true,"operation":provider.create().map_err(|e|e.to_string())?.to_string()}),
            ),
            "steam.update" => {
                let mut args = args;
                if let Some(id) = args["id"].as_str() {
                    let parsed = id.parse::<u64>().map_err(|_| "Invalid item ID")?;
                    args["id"] = json!(parsed);
                }
                let update: ItemUpdate = serde_json::from_value(args).map_err(|e| e.to_string())?;
                let call = provider.update(&update).map_err(|e| e.to_string())?;
                Ok(json!({"pending":true,"operation":call.to_string()}))
            }
            _ => Err(format!("Unknown Steamworks command: {method}")),
        }
    }
    pub(super) fn pump_steam(&self) {
        let mut state = self.ivars().borrow_mut();
        let Some(provider) = &mut state.steam else {
            return;
        };
        let events = provider.pump();
        let mut roots = vec![];
        for event in events {
            if event.ok
                && event.kind == "download"
                && let Some(path) = event
                    .value
                    .pointer("/install/folder")
                    .and_then(Value::as_str)
            {
                roots.push(PathBuf::from(path));
            }
            if !event.ok {
                append_bounded(
                    &mut state.report,
                    &format!("\nSteam {}: {}", event.kind, event.value),
                );
            }
            if state.steam_events.len() >= 128 {
                state.steam_events.pop_front();
            }
            state.steam_events.push_back(event);
        }
        let mut changed = false;
        for root in roots {
            if root.is_dir() && !state.service.config.library_roots.contains(&root) {
                state.service.config.library_roots.push(root);
                changed = true;
            }
        }
        if changed {
            state.service.refresh();
            if let Err(error) = state.service.save() {
                append_bounded(&mut state.report, &error.to_string());
            }
        }
    }
}

fn string_array(value: &Value) -> Result<Vec<String>, String> {
    if value.is_null() {
        return Ok(vec![]);
    }
    value
        .as_array()
        .ok_or("Tags must be an array")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "Tag must be a string".into())
        })
        .collect()
}
