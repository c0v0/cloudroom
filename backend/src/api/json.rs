//! JSON views for API responses.

use serde_json::{json, Value};

use crate::api::AppState;
use crate::config::Config;
use crate::domain::models::*;

// ----------------------------------------------------------- JSON helpers

pub fn user_json(u: &User, _cfg: &Config) -> Value {
    json!({
        "user_id": u.user_id,
        "username": u.username,
        "role": u.role.as_str(),
        "avatar_url": u.avatar_id.as_ref().map(|a| format!("/api/v1/avatars/{a}")),
        "created_at": u.created_at.to_rfc3339(),
        "disabled": u.disabled,
    })
}

pub fn member_json(m: &RoomMember, user: Option<&User>) -> Value {
    json!({
        "user_id": m.user_id,
        "username": user.and_then(|u| u.username.as_ref()),
        "avatar_url": user.and_then(|u| u.avatar_id.as_ref()).map(|id| format!("/api/v1/avatars/{id}")),
        "nickname": m.nickname,
        "room_role": m.room_role.as_str(),
        "joined_at": m.joined_at.to_rfc3339(),
        "last_online_at": m.last_online_at.map(|d| d.to_rfc3339()),
    })
}

pub fn room_json(r: &Room, my_role: Option<&str>) -> Value {
    json!({
        "code": r.room_code,
        "kind": r.kind.as_str(),
        "state": r.state.as_str(),
        "owner_user_id": r.owner_user_id,
        "created_at": r.created_at.to_rfc3339(),
        "expires_at": r.expires_at.map(|d| d.to_rfc3339()),
        "allow_direct_messages": r.allow_direct_messages,
        "text_enabled": r.text_enabled,
        "media_mode": r.media_mode.as_str(),
        "audio_bitrate": r.audio_bitrate,
        "room_muted": r.room_muted,
        "announcement": r.announcement,
        "max_members": r.max_members,
        "last_seq": r.last_seq,
        "e2ee": r.e2ee,
        "attachments_enabled": r.attachments_enabled,
        "my_role": my_role,
    })
}

pub fn message_json(m: &Message, sender: Option<&User>, nickname: Option<&str>) -> Value {
    let deleted = m.deleted_at.is_some();
    json!({
        "id": m.message_id,
        "seq": m.seq,
        "sender": {
            "user_id": m.sender_user_id,
            "username": sender.and_then(|u| u.username.as_ref()),
            "nickname": nickname,
        },
        // 已删除消息不下发正文，前端渲染占位。
        "body": if deleted { "" } else { m.body.as_str() },
        "reply": m.reply_snapshot.as_ref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null),
        "created_at": m.created_at.to_rfc3339(),
        "edited_at": m.edited_at.map(|d| d.to_rfc3339()),
        "deleted": deleted,
        "attachment_id": m.attachment_id,
        "attachment": Value::Null,
    })
}

/// 附件元数据（图片/文件）。
pub fn attachment_json(a: &Attachment, code: &str) -> Value {
    json!({
        "id": a.attachment_id,
        "filename": a.filename,
        "mime": a.mime,
        "size": a.size,
        "url": format!("/api/v1/rooms/{code}/attachments/{}", a.attachment_id),
    })
}

pub fn room_request_json(r: &RoomRequest) -> Value {
    json!({
        "request_id": r.request_id,
        "requester_id": r.requester_id,
        "kind": r.request_kind,
        "note": r.note,
        "desired_code": r.desired_code,
        "target_room_code": Value::Null,
        "media_mode": r.media_mode.as_str(),
        "audio_bitrate": r.audio_bitrate,
        "attachments": r.attachments,
        "status": r.status,
        "created_at": r.created_at.to_rfc3339(),
        "decided_at": r.decided_at.map(|d| d.to_rfc3339()),
        "decided_by": r.decided_by,
        "decision_reason": r.decision_reason,
        "created_room_code": Value::Null,
    })
}

/// 请求列表展示：补充申请人用户名（admin 视图）与切换申请的目标房间码。
pub async fn requests_json(state: &AppState, rows: &[RoomRequest]) -> Vec<Value> {
    let mut out = Vec::new();
    for r in rows {
        let mut v = room_request_json(r);
        if let Some(obj) = v.as_object_mut() {
            if let Some(rid) = &r.target_room_id {
                let code = state
                    .db
                    .with_sync(|conn| crate::infra::repo::get_room_by_id(conn, rid))
                    .ok()
                    .flatten()
                    .map(|room| room.room_code);
                if let Some(code) = code {
                    obj.insert("target_room_code".into(), json!(code));
                }
            }
            let username = state
                .db
                .with_sync(|c| crate::infra::repo::get_user(c, &r.requester_id))
                .ok()
                .flatten()
                .and_then(|u| u.username)
                .unwrap_or_default();
            obj.insert("requester_username".into(), json!(username));
            // 房间号：自定义码或自动生成占位
            if obj
                .get("desired_code")
                .and_then(|d| d.as_str())
                .map(|s| s.is_empty())
                .unwrap_or(true)
            {
                obj.insert("desired_code".into(), Value::Null);
            }
        }
        out.push(v);
    }
    out
}

pub fn invite_json(i: &Invite, raw_token: Option<&str>) -> Value {
    json!({
        "invite_id": i.invite_id,
        "kind": i.kind,
        "room_code": None::<String>,
        "max_role": i.max_role,
        "created_at": i.created_at.to_rfc3339(),
        "expires_at": i.expires_at.to_rfc3339(),
        "consumed_at": i.consumed_at.map(|d| d.to_rfc3339()),
        "consumed_by": i.consumed_by,
        "revoked_at": i.revoked_at.map(|d| d.to_rfc3339()),
        "active": i.active(chrono::Utc::now()),
        "token": raw_token,
    })
}

pub fn audit_json(a: &AuditEntry) -> Value {
    json!({
        "entry_id": a.entry_id,
        "actor_user_id": a.actor_user_id,
        "action": a.action,
        "target_type": a.target_type,
        "target_id": a.target_id,
        "detail": serde_json::from_str::<Value>(&a.detail).unwrap_or(Value::Null),
        "created_at": a.created_at.to_rfc3339(),
    })
}
