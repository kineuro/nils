// SPDX-License-Identifier: AGPL-3.0-only

//! Counts kept and answered at once (2026-11-12), for the doors a page
//! asks on arrival: a dataset's summary, the sources door's counts of each
//! dataset and the cohorts' list. It is the way the dataset summary has
//! kept its counts since 2026-10-17, taken out of that module so that the
//! other doors keep theirs the same way and not by a second one.
//!
//! A thing's counts are kept in a slot under a stamp of what they were read
//! from. A request is answered from the slot whatever its age. Where the
//! stamp moved, a door wrote since ([`Keep::forget`]) or the keep ran out,
//! one reading runs apart, on a thread and a registry connection of its
//! own, while the requests keep answering the kept counts, said to be
//! counted again (`counting`) with when they were counted (`as_of`). No
//! second reading of a slot starts while one runs, and a reading of one
//! slot holds up no request for another. Only a thing never counted makes
//! a request wait, once, with the others that ask meanwhile, or answers
//! that its first count runs where the caller does not wait.
//!
//! The counts outlive a start of the engine: each reading is written to
//! `registry_meta` under its stamp, and the first request after a start
//! takes that copy over, so it counts again, apart, only where the stamp
//! moved. A stamp never names the engine's version, only a number of the
//! counting itself, so an update counts nothing again by itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use nils_registry::Registry;

/// Counts as kept: under the stamp they were read under, when they were
/// read (`as_of`, which the answer says), how long they are kept, and
/// whether a door wrote since (`stale`: answered still, and read again at
/// the next request).
struct Kept<T> {
    stamp: String,
    at: Instant,
    as_of: String,
    ttl: Duration,
    stale: bool,
    value: Arc<T>,
}

/// Counts as answered: the counts, when they were read, and whether they
/// are being read again now.
pub(crate) struct Answer<T> {
    /// None while the first reading of a thing with none kept runs, for a
    /// caller who does not wait for it.
    pub(crate) value: Option<Arc<T>>,
    pub(crate) as_of: String,
    pub(crate) counting: bool,
}

struct Slot<T> {
    kept: Option<Kept<T>>,
    reading: bool,
    /// How many readings were started, for the tests.
    reads: usize,
    /// Whether the counts the registry keeps were looked for, once after a
    /// start of the engine, and whether a request is looking now.
    loaded: bool,
    loading: bool,
    /// How many times a door put its own reading in the slot
    /// ([`Keep::amend`]): a reading that began before is not kept at all.
    renewed: u64,
}

impl<T> Default for Slot<T> {
    fn default() -> Slot<T> {
        Slot {
            kept: None,
            reading: false,
            reads: 0,
            loaded: false,
            loading: false,
            renewed: 0,
        }
    }
}

/// The counts of one kind of thing, by slot, and the signal a request
/// waiting for another's reading wakes on.
pub(crate) struct Keep<T> {
    slots: Mutex<HashMap<String, Slot<T>>>,
    over: Condvar,
    /// How many times the kept counts were forgotten: a reading that began
    /// before a door wrote may have read what the write changed, so what
    /// it read is kept stale, answered and read again at the next request.
    forgotten: AtomicU64,
}

impl<T> Default for Keep<T> {
    fn default() -> Keep<T> {
        Keep {
            slots: Mutex::new(HashMap::new()),
            over: Condvar::new(),
            forgotten: AtomicU64::new(0),
        }
    }
}

/// What a request asks a keep.
pub(crate) struct Ask<'a> {
    /// The slot: the thing counted, as this engine names it.
    pub(crate) slot: &'a str,
    /// The `registry_meta` key its counts are kept under between starts.
    pub(crate) stored: &'a str,
    /// What the counts are read under now.
    pub(crate) stamp: &'a str,
    /// How long a reading made now is kept while the stamp is the same.
    pub(crate) ttl: Duration,
    /// Kept at least ten times as long as the reading took (the sources
    /// door while a digest runs: every few seconds where a count is a few
    /// milliseconds, and never one count on the heels of another where it
    /// is ten seconds).
    pub(crate) paced: bool,
    /// Whether a thing never counted is waited for; where not, its first
    /// reading runs apart too and the answer has no counts until it ends.
    pub(crate) wait: bool,
    /// Read now, whatever is kept (a probe asked for at the sources door):
    /// the caller waits for a reading that begins after its own write.
    pub(crate) fresh: bool,
}

/// Counts read back from the registry.
pub(crate) struct Restored<T> {
    pub(crate) stamp: String,
    pub(crate) as_of: String,
    pub(crate) value: T,
}

/// One reading of a thing's counts: made by the request that starts it,
/// run by that request or on a thread of its own.
pub(crate) trait Reading<T>: Send + 'static {
    type Error;
    /// Read the counts from the registry.
    fn read(&mut self, registry: &mut Registry) -> Result<T, Self::Error>;
    /// The counts as the registry keeps them under `stamp`; none where
    /// they are not kept between starts.
    fn stored(&self, stamp: &str, as_of: &str, value: &T) -> Option<String>;
}

/// A reading under way: whatever becomes of it, the others are told it is
/// over, and one of them reads the counts where it read none.
struct Flight<T: 'static>(&'static Keep<T>, String);

impl<T> Drop for Flight<T> {
    fn drop(&mut self) {
        let mut slots = self.0.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = slots.get_mut(&self.1) {
            slot.reading = false;
        }
        self.0.over.notify_all();
    }
}

/// What a reading is kept under once it is over.
struct Under {
    slot: String,
    stored: String,
    stamp: String,
    ttl: Duration,
    paced: bool,
}

impl Under {
    fn of(ask: &Ask<'_>) -> Under {
        Under {
            slot: ask.slot.to_string(),
            stored: ask.stored.to_string(),
            stamp: ask.stamp.to_string(),
            ttl: ask.ttl,
            paced: ask.paced,
        }
    }
}

impl<T: Send + Sync + 'static> Keep<T> {
    /// Read the counts now and keep them under their stamp: stale where a
    /// door wrote meanwhile, since the reading may have read what the write
    /// changed. They are written to the registry too, for the next start of
    /// the engine; a failure to write them costs that start one reading and
    /// nothing else.
    fn read_and_keep<R: Reading<T>>(
        &self,
        registry: &mut Registry,
        under: &Under,
        mut reading: R,
    ) -> Result<Answer<T>, R::Error> {
        let as_of = nils_registry::time::now_iso();
        let forgotten = self.forgotten.load(Ordering::SeqCst);
        let renewed = self.renewed_of(&under.slot);
        let began = Instant::now();
        let value = Arc::new(reading.read(registry)?);
        let took = began.elapsed();
        let answer = Answer {
            value: Some(Arc::clone(&value)),
            as_of: as_of.clone(),
            counting: false,
        };
        {
            let mut held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            // a door put its own reading there meanwhile: what this reading
            // read may be from before the door's write, and the next
            // request reads again
            let slot = held.entry(under.slot.clone()).or_default();
            if slot.renewed != renewed {
                return Ok(answer);
            }
            slot.kept = Some(Kept {
                stamp: under.stamp.clone(),
                at: Instant::now(),
                as_of: as_of.clone(),
                ttl: if under.paced {
                    under.ttl.max(took * 10)
                } else {
                    under.ttl
                },
                stale: self.forgotten.load(Ordering::SeqCst) != forgotten,
                value: Arc::clone(&value),
            });
        }
        if let Some(text) = reading.stored(&under.stamp, &as_of, &value) {
            let _ = nils_registry::home::set_meta(registry.store(), &under.stored, &text);
        }
        Ok(answer)
    }

    /// Read the counts on a thread of its own, on a connection of its own
    /// to the registry `home` names, while the requests answer the kept
    /// ones; `flight` is let go when it is over, whatever becomes of it,
    /// and where no thread starts it is let go at once and the next request
    /// reads them apart again.
    fn read_apart<R: Reading<T>>(
        &'static self,
        home: nils_registry::Home,
        under: Under,
        reading: R,
        flight: Flight<T>,
    ) {
        let _ = std::thread::Builder::new()
            .name("kept-counts".to_string())
            .spawn(move || {
                let _flight = flight;
                if let Ok(mut registry) = home.open() {
                    let _ = self.read_and_keep(&mut registry, &under, reading);
                }
            });
    }

    /// A thing's counts: the kept ones at once, whatever their age, said to
    /// be counted again (`counting`) while a reading runs apart; a reading
    /// is started where what they were read under moved, a door wrote since
    /// or their keep ran out, and none while one runs. Only a thing with no
    /// kept counts at all waits for its first reading, and the requests at
    /// once with it wait for the same one; unless the caller does not
    /// `wait`: then that first reading runs apart too, and the answer has
    /// no counts until it is over.
    ///
    /// `restore` reads the registry's copy back, asked once after a start
    /// of the engine by the first request, which the ones at once with it
    /// wait for; `reading` makes the reading where this request starts one.
    pub(crate) fn answer<R: Reading<T>>(
        &'static self,
        registry: &mut Registry,
        ask: &Ask<'_>,
        restore: impl FnOnce(&str) -> Option<Restored<T>>,
        reading: impl FnOnce() -> R,
    ) -> Result<Answer<T>, R::Error> {
        let mut restore = Some(restore);
        let mut held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if ask.fresh {
            // a reading under way began before this request's write
            while held.get(ask.slot).is_some_and(|s| s.reading) {
                held = self.over.wait(held).unwrap_or_else(|e| e.into_inner());
            }
            let slot = held.entry(ask.slot.to_string()).or_default();
            slot.loaded = true;
            slot.reading = true;
            slot.reads += 1;
            drop(held);
            let flight = Flight(self, ask.slot.to_string());
            let answer = self.read_and_keep(registry, &Under::of(ask), reading())?;
            drop(flight);
            return Ok(answer);
        }
        loop {
            let slot = held.entry(ask.slot.to_string()).or_default();
            if !slot.loaded {
                if slot.loading {
                    held = self.over.wait(held).unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                slot.loading = true;
                drop(held);
                let stored = nils_registry::home::get_meta(registry.store(), ask.stored)
                    .ok()
                    .flatten()
                    .and_then(|text| restore.take().and_then(|r| r(&text)));
                held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
                let slot = held.entry(ask.slot.to_string()).or_default();
                slot.loading = false;
                slot.loaded = true;
                if slot.kept.is_none() {
                    slot.kept = stored.map(|s| Kept {
                        stamp: s.stamp,
                        at: Instant::now(),
                        as_of: s.as_of,
                        ttl: ask.ttl,
                        stale: false,
                        value: Arc::new(s.value),
                    });
                }
                self.over.notify_all();
                continue;
            }
            if let Some(k) = &slot.kept {
                let answer = Answer {
                    value: Some(Arc::clone(&k.value)),
                    as_of: k.as_of.clone(),
                    counting: true,
                };
                if !k.stale && k.stamp == ask.stamp && k.at.elapsed() < k.ttl {
                    return Ok(Answer {
                        counting: false,
                        ..answer
                    });
                }
                if slot.reading {
                    return Ok(answer);
                }
                slot.reading = true;
                slot.reads += 1;
                drop(held);
                let flight = Flight(self, ask.slot.to_string());
                self.read_apart(registry.home().clone(), Under::of(ask), reading(), flight);
                return Ok(answer);
            }
            // none kept: a caller who does not wait is answered at once,
            // the first reading running apart
            if !ask.wait {
                let pending = Answer {
                    value: None,
                    as_of: String::new(),
                    counting: true,
                };
                if slot.reading {
                    return Ok(pending);
                }
                slot.reading = true;
                slot.reads += 1;
                drop(held);
                let flight = Flight(self, ask.slot.to_string());
                self.read_apart(registry.home().clone(), Under::of(ask), reading(), flight);
                return Ok(pending);
            }
            if !slot.reading {
                slot.reading = true;
                slot.reads += 1;
                break;
            }
            held = self.over.wait(held).unwrap_or_else(|e| e.into_inner());
        }
        drop(held);
        let flight = Flight(self, ask.slot.to_string());
        let answer = self.read_and_keep(registry, &Under::of(ask), reading())?;
        drop(flight);
        Ok(answer)
    }

    /// Mark every kept count stale: a write at a door may have changed
    /// them, so they are read again at the next request, which they still
    /// answer.
    pub(crate) fn forget(&self) {
        let mut held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        self.forgotten.fetch_add(1, Ordering::SeqCst);
        for slot in held.values_mut() {
            if let Some(k) = &mut slot.kept {
                k.stale = true;
            }
        }
    }

    /// How many times a door wrote the slot itself.
    fn renewed_of(&self, slot: &str) -> u64 {
        let held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        held.get(slot).map_or(0, |s| s.renewed)
    }

    /// Put a door's own reading in a slot's kept counts: the door changed
    /// a part of what they count and read that part again, so the next
    /// request answers it while the whole is read again apart. A reading
    /// under way, which began before the write, is not kept. Nothing where
    /// the slot keeps no counts.
    pub(crate) fn amend(&self, slot: &str, with: impl FnOnce(&T) -> T) {
        let mut held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let slot = held.entry(slot.to_string()).or_default();
        slot.renewed += 1;
        if let Some(k) = &mut slot.kept {
            k.value = Arc::new(with(&k.value));
            k.stale = true;
        }
    }

    /// Wait until no reading of the slot runs, for the tests.
    #[cfg(test)]
    pub(crate) fn settled(&self, slot: &str) {
        let began = Instant::now();
        let mut held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        while held.get(slot).is_some_and(|s| s.reading) {
            assert!(
                began.elapsed() < Duration::from_secs(60),
                "a reading never ended"
            );
            held = self
                .over
                .wait_timeout(held, Duration::from_millis(200))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Drop the slot's kept counts, as a start of the engine leaves them,
    /// for the tests: the registry's copy is looked for again.
    #[cfg(test)]
    pub(crate) fn dropped(&self, slot: &str) {
        let mut held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = held.get_mut(slot) {
            slot.kept = None;
            slot.loaded = false;
        }
    }

    /// How many readings of the slot were started, for the tests.
    #[cfg(test)]
    pub(crate) fn reads_of(&self, slot: &str) -> usize {
        let held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        held.get(slot).map_or(0, |s| s.reads)
    }

    /// Run the slot's keep out, as time does, for the tests.
    #[cfg(test)]
    pub(crate) fn aged(&self, slot: &str) {
        let mut held = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = held.get_mut(slot).and_then(|s| s.kept.as_mut()) {
            k.ttl = Duration::ZERO;
        }
    }
}
