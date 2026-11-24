//! Number claims and their durable replies from the current main computer.

use super::*;

#[derive(Clone)]
pub(in crate::queue) struct Grant {
    claimed: Vec<[u64; 2]>,
    granted: Vec<[u64; 2]>,
}

impl Grant {
    pub fn accepts(&self, number: u64) -> Option<bool> {
        contains(&self.claimed, number).then(|| contains(&self.granted, number))
    }
}

pub(in crate::queue) fn contains(ranges: &[[u64; 2]], number: u64) -> bool {
    ranges
        .iter()
        .any(|[from, to]| (*from..=*to).contains(&number))
}

pub(super) fn grant(asking: &Value, block: &Value) -> Grant {
    Grant {
        claimed: ranges(asking.get("claims")),
        granted: ranges(block.get("granted")),
    }
}

pub(in crate::queue) fn runs(numbers: Vec<u64>) -> Vec<[u64; 2]> {
    joined(
        numbers
            .into_iter()
            .filter(|number| (1..=MAX).contains(number))
            .map(|number| [number, number])
            .collect(),
    )
}

impl Numbers {
    pub async fn used(&self) -> Result<Vec<[u64; 2]>, Error> {
        let path = self.file.clone();
        tokio::task::spawn_blocking(move || {
            LEDGER
                .read(&path)
                .map(|record| ranges(record.as_ref().and_then(|record| record.fields.get("used"))))
                .map_err(Error::from)
        })
        .await
        .map_err(io::Error::other)?
    }

    pub async fn asking_claims(&self) -> Result<Vec<[u64; 2]>, Error> {
        let path = self.file.clone();
        tokio::task::spawn_blocking(move || {
            LEDGER
                .read(&path)
                .map(|record| {
                    ranges(
                        record
                            .as_ref()
                            .and_then(|record| record.fields.get("asking"))
                            .and_then(|ask| ask.get("claims")),
                    )
                })
                .map_err(Error::from)
        })
        .await
        .map_err(io::Error::other)?
    }

    pub async fn claim(
        &self,
        numbers: Vec<u64>,
        restores: Vec<u64>,
        high: u64,
        now: i64,
    ) -> Result<Option<Grant>, Error> {
        let claimed = runs(numbers);
        if claimed.is_empty() {
            return Ok(None);
        }
        self.change(high, move |ledger, scope| {
            let ready = !scope.setup.as_ref().is_some_and(|setup| setup.merging);
            if ready
                && scope.main()
                && let Some(ceiling) = ledger.ceiling(scope.setup.as_ref(), scope.lease)
                && ledger.claim_room(&claimed, 0, ceiling)
            {
                let granted = ledger.grant_claims("self", &claimed, None, ceiling);
                return Ok(Some(Grant { claimed, granted }));
            }
            let high = claimed.last().expect("nonempty claim")[1];
            for [from, _] in &mut ledger.leases {
                *from = (*from).max(high + 1);
            }
            ledger.leases.retain(|[from, to]| from <= to);
            ledger.fields.insert("leases".into(), json!(ledger.leases));
            let asking = ledger.fields.get("asking");
            let restores = joined(
                ranges(asking.and_then(|ask| ask.get("restores")))
                    .into_iter()
                    .chain(intersect(&runs(restores), &claimed))
                    .collect(),
            );
            let claims = joined(
                ranges(asking.and_then(|ask| ask.get("claims")))
                    .into_iter()
                    .chain(claimed)
                    .collect(),
            );
            let mut ask = json!({"id":ask_id()?,"count":BLOCK,"since":now,"claims":claims});
            if !restores.is_empty() {
                ask["restores"] = json!(restores);
            }
            ledger.fields.insert("asking".into(), ask);
            Ok(None)
        })
        .await
    }
}

impl Ledger {
    pub(super) fn note_used(&mut self, numbers: Vec<u64>) {
        if !numbers.is_empty() {
            self.used.extend(runs(numbers));
            self.fields
                .insert("used".into(), json!(joined(self.used.clone())));
        }
    }

    pub(super) fn claim_room(&mut self, claims: &[[u64; 2]], count: u64, ceiling: u64) -> bool {
        let next = self.next();
        let needed = intersect(claims, &[[next, (next + 10_000).min(MAX - count)]])
            .last()
            .map_or(next - 1, |claim| claim[1])
            + count;
        if needed <= ceiling {
            return true;
        }
        let before = self
            .fields
            .get("reserveThrough")
            .and_then(integer)
            .unwrap_or(0);
        self.fields
            .insert("reserveThrough".into(), before.max(needed).into());
        false
    }

    pub(super) fn grant_claims(
        &mut self,
        by: &str,
        claims: &[[u64; 2]],
        owned: Option<&[[u64; 2]]>,
        ceiling: u64,
    ) -> Vec<[u64; 2]> {
        let next = self.next();
        let mut granted = self
            .fields
            .get("granted")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let before = ranges(granted.get(by));
        let mut offered = intersect(claims, &[[next, (next + 10_000).min(ceiling)]]);
        offered.extend(intersect(claims, &before));
        if let Some(_restores) = owned.filter(|_| by != "self") {
            let blocks: Vec<_> = self
                .fields
                .get("given")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|given| given.values())
                .filter(|block| block["by"].as_str() == Some(by))
                .filter_map(|block| Some([integer(&block["from"])?, integer(&block["to"])?]))
                .collect();
            offered.extend(intersect(claims, &blocks));
        }
        let offered = joined(offered);
        if let Some([_, last]) = offered.last() {
            self.fields.insert("next".into(), next.max(last + 1).into());
            granted.insert(
                by.into(),
                json!(joined(
                    before.into_iter().chain(offered.iter().copied()).collect()
                )),
            );
            self.fields.insert("granted".into(), granted.into());
        }
        offered
    }
}
