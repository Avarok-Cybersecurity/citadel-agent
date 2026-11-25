//! What an applied op writes (`vfs-ops.md` R3, R4, R8, R9; `vfs-outsiders.md` §2, §3).

use super::tree::{Admission, Entry, Place, Usage};
use super::{Position, Reg, Tree, Won};
use crate::authorize::{admission_charged, VolumeCtx};
use crate::inode::InodeId;
use crate::limits::RATE_WINDOW_MS;
use crate::model::{AdmittedKey, ClaimWhat, Stamped};
use crate::op::{Body, Op};
use crate::principal::Principal;
use std::collections::{BTreeMap, BTreeSet};

impl Tree {
    pub(super) fn effects(&mut self, position: Position, op: &Op, ctx: Option<&VolumeCtx>) {
        let Some(body) = &op.body else {
            return;
        };
        let header = &op.header;
        let won = Won {
            position,
            stamp: header.stamp(),
            at_ms: header.at_ms,
        };
        let target = header.target;
        let charge = ctx.and_then(|ctx| admission_charged(self, ctx, op));
        let created = matches!(body, Body::Create { .. }) && !self.entries.contains_key(&target);
        let stamped = Stamped {
            by: header.author.clone(),
            at_ms: header.at_ms,
        };
        match body {
            Body::Create {
                kind,
                parent,
                name,
                pointee,
                mode,
                owner,
            } => {
                let place = Place {
                    parent: *parent,
                    name: name.clone(),
                };
                if let Some(entry) = self.entries.get_mut(&target) {
                    entry.pointee = reg(pointee.clone(), &won);
                    entry.owner = reg(owner.clone(), &won);
                    entry.mode = reg(*mode, &won);
                    self.set_place(target, reg(place, &won));
                } else {
                    let entry = Entry {
                        id: target,
                        kind: *kind,
                        created: stamped,
                        place: reg(place, &won),
                        pointee: reg(pointee.clone(), &won),
                        owner: reg(owner.clone(), &won),
                        mode: reg(*mode, &won),
                        removal: None,
                        admissions: BTreeMap::new(),
                    };
                    self.children.entry(*parent).or_default().insert(target);
                    self.entries.insert(target, entry);
                }
            }
            Body::Place { parent, name } => self.set_place(
                target,
                reg(
                    Place {
                        parent: *parent,
                        name: name.clone(),
                    },
                    &won,
                ),
            ),
            Body::SetPointee { pointee } => {
                self.with(target, |e| e.pointee = reg(pointee.clone(), &won))
            }
            Body::Remove => self.with(target, |e| e.removal = Some(reg(Some(stamped), &won))),
            Body::Restore => self.with(target, |e| e.removal = Some(reg(None, &won))),
            Body::Purge => self.purge(target),
            Body::Chmod { mode } => self.with(target, |e| e.mode = reg(*mode, &won)),
            Body::Chown { owner } => self.with(target, |e| e.owner = reg(owner.clone(), &won)),
            Body::Admit {
                who,
                expires_ms,
                caps,
            } => {
                let key = who.key();
                self.with(target, |e| {
                    let kept = e
                        .admissions
                        .get(&key)
                        .and_then(|r| r.value.clone())
                        .filter(|a| a.who == *who);
                    let kept = kept.as_ref();
                    let admission = Admission {
                        who: who.clone(),
                        by: header.author.clone(),
                        expires_ms: *expires_ms,
                        caps: *caps,
                        claims: kept.map(|a| a.claims.clone()).unwrap_or_default(),
                        handed_over: kept.map(|a| a.handed_over.clone()).unwrap_or_default(),
                        used: kept.map(|a| a.used.clone()).unwrap_or_default(),
                    };
                    e.admissions.insert(key, reg(Some(admission), &won));
                })
            }
            Body::Dismiss { who, .. } => self.with(target, |e| {
                e.admissions.insert(*who, reg(None, &won));
            }),
            Body::Claim { link_id, what } => {
                let Principal::LinkGuest { key_fp, .. } = &header.author else {
                    return;
                };
                let key = AdmittedKey::Link { link_id: *link_id };
                self.with(target, |e| claim(e, &key, key_fp, what, &won));
            }
        }
        if let Some((root, key)) = charge {
            let bytes = match body {
                Body::Create { pointee, .. } | Body::SetPointee { pointee } => pointee.byte_len(),
                _ => 0,
            };
            self.with(root, |e| {
                if let Some(Some(a)) = e.admissions.get_mut(&key).map(|r| r.value.as_mut()) {
                    charge_usage(&mut a.used, bytes, created, header.at_ms);
                }
            });
        }
    }

    fn with(&mut self, id: InodeId, f: impl FnOnce(&mut Entry)) {
        if let Some(entry) = self.entries.get_mut(&id) {
            f(entry);
        }
    }

    fn set_place(&mut self, id: InodeId, place: Reg<Place>) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        let old = entry.place.value.parent;
        let new = place.value.parent;
        entry.place = place;
        if old != new {
            if let Some(siblings) = self.children.get_mut(&old) {
                siblings.remove(&id);
            }
            self.children.entry(new).or_default().insert(id);
        }
    }

    /// R9: the entry leaves the state; a marker holding only its id remains.
    fn purge(&mut self, id: InodeId) {
        if let Some(entry) = self.entries.remove(&id) {
            if let Some(siblings) = self.children.get_mut(&entry.place.value.parent) {
                siblings.remove(&id);
            }
        }
        self.children.remove(&id);
        self.purged.insert(id);
    }
}

fn reg<T>(value: T, won: &Won) -> Reg<T> {
    Reg {
        value,
        won: won.clone(),
    }
}

fn claim(entry: &mut Entry, key: &AdmittedKey, key_fp: &str, what: &ClaimWhat, won: &Won) {
    let Some(Some(link)) = entry.admissions.get_mut(key).map(|r| r.value.as_mut()) else {
        return;
    };
    match what {
        ClaimWhat::Key { .. } => {
            if !link.claims.iter().any(|k| k == key_fp) {
                link.claims.push(key_fp.to_string());
            }
        }
        ClaimWhat::Handover { cid, id_fp } => {
            link.handed_over.insert(key_fp.to_string());
            let member = Admission {
                who: crate::model::Admitted::Member {
                    cid: *cid,
                    id_fp: id_fp.clone(),
                },
                by: link.by.clone(),
                expires_ms: link.expires_ms,
                caps: link.caps,
                claims: Vec::new(),
                handed_over: BTreeSet::new(),
                used: Usage::default(),
            };
            let reg = Reg {
                value: Some(member),
                won: won.clone(),
            };
            entry
                .admissions
                .insert(AdmittedKey::Member { cid: *cid }, reg);
        }
    }
}

fn charge_usage(used: &mut Usage, bytes: u64, created: bool, at_ms: u64) {
    used.bytes = used.bytes.saturating_add(bytes);
    if created {
        used.entries = used.entries.saturating_add(1);
    }
    used.recent_ms
        .retain(|t| t.saturating_add(RATE_WINDOW_MS) > at_ms);
    used.recent_ms.push(at_ms);
}
