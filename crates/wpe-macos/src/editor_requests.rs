//! Settle native editor requests outside the document borrow. Nested host
//! requests are drained too; failures always become explicit completion events.
use serde_json::Value;
use wpe_editor::{EditorReply, HostRequest};
pub fn settle_requests(
    mut pending: impl FnMut() -> Result<Vec<HostRequest>, String>,
    mut invoke: impl FnMut(&HostRequest) -> Result<Value, String>,
    mut complete: impl FnMut(u64, Result<Value, String>) -> Result<EditorReply, String>,
) -> Result<Vec<EditorReply>, String> {
    let mut replies = vec![];
    let mut count = 0;
    loop {
        let requests = pending()?;
        if requests.is_empty() {
            return Ok(replies);
        }
        for request in requests {
            let value = if count < 64 {
                count += 1;
                invoke(&request)
            } else {
                Err("Native host request chain exceeds64operations".into())
            };
            replies.push(complete(request.request_id, value)?);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{cell::RefCell, rc::Rc};
    #[test]
    fn conditional_and_nested_requests_end_in_real_completion_or_error() {
        let root = std::env::temp_dir().join(format!("wpe-native-editor-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let editor = Rc::new(RefCell::new(
            wpe_editor::Editor::new(
                root.clone(),
                json!({"type":"scene","file":"scene.json"}),
                json!({"objects":[]}),
            )
            .unwrap(),
        ));
        editor
            .borrow_mut()
            .execute("runScriptPreview", json!([]))
            .unwrap();
        let replies = settle_requests(
            || Ok(editor.borrow().pending_host_requests()),
            |request| {
                if request.method == "runScriptPreview" {
                    editor
                        .borrow_mut()
                        .execute("isDLCInstalled", json!([123]))
                        .unwrap();
                    Ok(json!({"preview":"owned fixture"}))
                } else {
                    Err("SDK unavailable in fixture".into())
                }
            },
            |id, value| {
                editor
                    .borrow_mut()
                    .complete_host_request(id, value)
                    .map_err(|e| e.to_string())
            },
        )
        .unwrap();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0].value["ok"], true);
        assert_eq!(replies[1].value["ok"], false);
        assert!(editor.borrow().pending_host_requests().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
