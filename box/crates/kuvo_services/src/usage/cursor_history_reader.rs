//! The account-wide Cursor history shares the plan reader's access exchange and
//! hold. No browser process, private request body or provider credential is kept.
use super::*;
use crate::usage::{cursor_history_store as store, insights};
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use std::time::Duration;

struct Token {
    access: String,
    renewed: bool,
}
enum DayFailure {
    Invalid,
    Failed,
    Held,
}
impl Readers {
    pub(super) async fn cursor_sign_in_state(&self, account: &Account) -> Arc<Mutex<CursorSignIn>> {
        let state = self
            .cursor
            .lock()
            .await
            .entry(account.provider.clone())
            .or_insert_with(|| Arc::new(Mutex::new(CursorSignIn::default())))
            .clone();
        {
            let mut sign = state.lock().await;
            if sign.mark != account.sign_in {
                *sign = CursorSignIn {
                    mark: account.sign_in.clone(),
                    ..Default::default()
                };
            }
        }
        state
    }
    pub fn cursor_history_available(&self) -> bool {
        !self.isolated
            || self.fixture.is_some() && std::env::var_os("KUVO_TEST_CURSOR_HISTORY").is_some()
    }
    pub async fn refresh_cursor_history(&self, index: &insights::Service) {
        if !self.cursor_history_available() {
            return;
        }
        let at = now();
        let Ok(list) = self.accounts.all().await else {
            return;
        };
        let reader = self.clone();
        let Ok((found, unreadable)) = tokio::task::spawn_blocking(move || {
            let mut found = vec![];
            let mut unreadable = BTreeSet::new();
            for record in list.into_iter().filter(|r| r.kind == AgentKind::Cursor) {
                let id = record.provider().to_string();
                match reader.candidate(record, at) {
                    Ok(Some(candidate)) => found.push(candidate),
                    Ok(None) => {}
                    Err(_) => {
                        unreadable.insert(id);
                    }
                }
            }
            (found, unreadable)
        })
        .await
        else {
            return;
        };
        let mut providers = unreadable
            .into_iter()
            .filter(|id| id == "cursor" || id.starts_with("cursor@"))
            .collect::<BTreeSet<_>>();
        for source in found {
            let Credential::Cursor { key } = &source.credential else {
                continue;
            };
            providers.insert(source.account.provider.clone());
            let service = index.clone();
            let provider = source.account.provider.clone();
            let mark = source.account.sign_in.clone();
            let _ = tokio::task::spawn_blocking(move || {
                service
                    .forget_cursor_signin(&provider, &mark)
                    .map_err(|e| e.to_string())
            })
            .await;
            if index.cursor_held(&source.account, at) {
                continue;
            }
            if let Err(error) = self.read_cursor_history(index, &source, key, at).await {
                eprintln!(
                    "Couldn't keep {}'s account-wide usage: {error}",
                    source.account.provider
                );
            }
        }
        let service = index.clone();
        let _ = tokio::task::spawn_blocking(move || {
            service
                .retain_cursor_accounts(providers)
                .map_err(|e| e.to_string())
        })
        .await;
    }
    async fn read_cursor_history(
        &self,
        index: &insights::Service,
        source: &Candidate,
        key: &str,
        at: u64,
    ) -> Result<(), String> {
        let sign = self.cursor_sign_in_state(&source.account).await;
        let access = {
            let mut state = sign.lock().await;
            match self.cursor_access(&mut state, key, at, false).await {
                Ok(token) => token,
                Err(_) => return Ok(()),
            }
        };
        let user = jwt_subject(&access)
            .map(|s| s.split('|').nth(1).unwrap_or(&s).to_owned())
            .filter(|s| !s.is_empty())
            .ok_or("Cursor sign-in does not identify a user")?;
        let who = identity("cursor", &user);
        if index.peer_reads_whole("cursor", &who, at as i64) {
            return Ok(());
        }
        let mut hash = Sha256::new();
        hash.update(b"kuvo-code cursor history v1\n");
        hash.update(&user);
        let user_key = hex::encode(&hash.finalize()[..8]);
        let token = Arc::new(Mutex::new(Token {
            access,
            renewed: false,
        }));
        let now_ms = at as i64;
        let today = now_ms.div_euclid(store::DAY) * store::DAY;
        let service = index.clone();
        let user_copy = user_key.clone();
        let provider = source.account.provider.clone();
        let result = tokio::task::spawn_blocking(move || {
            service
                .cursor_days(&provider, &user_copy, now_ms)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
        let (mut kept, scanned) = result;
        if scanned.is_some_and(|last| {
            now_ms.saturating_sub(last) < MINUTE as i64
                && last.div_euclid(store::DAY) == today.div_euclid(store::DAY)
        }) {
            return Ok(());
        }
        let days = (0..store::DAYS)
            .map(|back| today - back * store::DAY)
            .collect::<Vec<_>>();
        let due = days
            .iter()
            .copied()
            .filter(|start| *start >= today - store::DAY || kept.get(start) != Some(&true))
            .collect::<Vec<_>>();
        let mut failed = false;
        let mut reads = stream::iter(due.into_iter().map(|start| {
            let token = token.clone();
            let sign = sign.clone();
            let user = &user;
            async move {
                (
                    start,
                    self.cursor_day(sign, token, key, user, start, now_ms).await,
                )
            }
        }))
        .buffer_unordered(4);
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            while let Some((start, result)) = reads.next().await {
                let day = match result {
                    Ok(day) => day,
                    Err(DayFailure::Held) => {
                        failed = true;
                        break;
                    }
                    Err(_) => {
                        failed = true;
                        continue;
                    }
                };
                kept.insert(start, day.complete);
                let service = index.clone();
                let user = user_key.clone();
                let saved = tokio::task::spawn_blocking(move || {
                    service
                        .save_cursor_day(&user, day, now_ms)
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?;
                saved?;
            }
            Ok::<_, String>(())
        })
        .await;
        match result {
            Ok(result) => result?,
            Err(_) => failed = true,
        }
        // A changed key may have been admitted while the pages were arriving.
        // Only the account that started this read can publish it.
        let readers = self.clone();
        let record = source.record.clone();
        let live = tokio::task::spawn_blocking(move || readers.candidate(record, now()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if !live.is_some_and(|live| live.account.sign_in == source.account.sign_in) {
            return Ok(());
        }
        let whole = !failed && days.iter().all(|day| kept.contains_key(day));
        let provider = source.account.provider.clone();
        let account = source.account.account.clone().unwrap_or("default".into());
        let label = source.account.label.clone();
        let mark = source.account.sign_in.clone();
        let service = index.clone();
        tokio::task::spawn_blocking(move || {
            service
                .keep_cursor_account(
                    &provider,
                    &account,
                    label.as_deref(),
                    &who,
                    &user_key,
                    whole.then_some(now_ms),
                    &mark,
                )
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(())
    }
    async fn cursor_day(
        &self,
        sign: Arc<Mutex<CursorSignIn>>,
        token: Arc<Mutex<Token>>,
        key: &str,
        user: &str,
        start: i64,
        now_ms: i64,
    ) -> Result<store::Day, DayFailure> {
        let end = (start + store::DAY).min(now_ms);
        let mut page = 1u64;
        let mut expected = None;
        let mut received = 0u64;
        let mut uses = BTreeMap::<(i64, String), crate::usage::native_logs::Counts>::new();
        let mut retried = false;
        loop {
            {
                let state = sign.lock().await;
                if state.held > now() || state.refused {
                    return Err(DayFailure::Held);
                }
            }
            let access = token.lock().await.access.clone();
            let request=self.client.post(self.endpoint("cursor/dashboard/get-filtered-usage-events","https://cursor.com/api/dashboard/get-filtered-usage-events"))
                .header("Cookie",format!("WorkosCursorSessionToken={user}%3A%3A{access}"))
                .timeout(Duration::from_secs(15))
                .json(&json!({"startDate":start.to_string(),"endDate":(end-1).to_string(),"page":page,"pageSize":1000}));
            let (status, body, retry) = self
                .send(request, "Cursor", now())
                .await
                .map_err(|_| DayFailure::Failed)?;
            if status == 429 {
                let mut state = sign.lock().await;
                Self::cursor_hold(&mut state, retry, now());
                return Err(DayFailure::Held);
            }
            if matches!(status, 401 | 403) && !retried {
                retried = true;
                let mut shared = token.lock().await;
                if !shared.renewed {
                    shared.renewed = true;
                    let mut state = sign.lock().await;
                    shared.access = self
                        .cursor_access(&mut state, key, now(), true)
                        .await
                        .map_err(|p| {
                            if matches!(p.kind, ProblemKind::RateLimited | ProblemKind::Expired) {
                                DayFailure::Held
                            } else {
                                DayFailure::Failed
                            }
                        })?;
                }
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(DayFailure::Failed);
            }
            let parsed = store::parse(&body).ok_or(DayFailure::Invalid)?;
            let (total, count) =
                store::merge_page(&mut uses, start, parsed).ok_or(DayFailure::Invalid)?;
            if expected.is_some_and(|prior| prior != total) {
                return Err(DayFailure::Invalid);
            }
            expected = Some(total);
            received = received.saturating_add(count);
            if received == total {
                return Ok(store::Day {
                    start,
                    complete: end == start + store::DAY,
                    uses: uses
                        .into_iter()
                        .map(|((slot, model), counts)| store::Use {
                            slot,
                            model,
                            counts,
                        })
                        .collect(),
                });
            }
            if received > total || count == 0 {
                return Err(DayFailure::Invalid);
            }
            page += 1;
        }
    }
}
