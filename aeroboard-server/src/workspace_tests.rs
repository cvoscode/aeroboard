#[cfg(test)]
mod tests {
    use crate::{
        app_state::AppState,
        catalog::{self, Catalog, RegisterRun},
        config::AppConfig,
        storage::FilesystemStore,
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use std::{collections::BTreeSet, sync::Arc};
    use tower::ServiceExt;
    use uuid::Uuid;

    fn registration(uid: &str, name: &str) -> RegisterRun {
        RegisterRun {
            run_uid: uid.into(),
            name: name.into(),
            project_id: "default".into(),
            tags: vec![],
            provenance: json!({}),
        }
    }
    async fn call(
        app: &axum::Router,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&bytes)})),
        )
    }
    fn state() -> AppState {
        let root = std::env::temp_dir().join(format!("aeroboard-workspace-{}", Uuid::new_v4()));
        let config = AppConfig::for_tests(root.join("data"), root.join("web"));
        AppState::new(
            config.clone(),
            Arc::new(FilesystemStore::new(config.data_dir).unwrap()),
        )
        .unwrap()
        .0
    }

    #[tokio::test]
    async fn registration_is_idempotent_and_concurrent_names_are_reserved() {
        let catalog = Catalog::open(std::path::Path::new("unused"), false).unwrap();
        let uid = Uuid::new_v4().to_string();
        let first = uid.clone();
        let record = catalog
            .write(move |db| catalog::register(db, registration(&first, "trial"), BTreeSet::new()))
            .await
            .unwrap();
        let first = uid.clone();
        let repeated = catalog
            .write(move |db| catalog::register(db, registration(&first, "trial"), BTreeSet::new()))
            .await
            .unwrap();
        assert_eq!(record.id, repeated.id);
        assert_eq!(record.revision, repeated.revision);
        let mut jobs = vec![];
        for _ in 0..8 {
            let catalog = catalog.clone();
            jobs.push(tokio::spawn(async move {
                catalog
                    .write(|db| {
                        catalog::register(
                            db,
                            registration(&Uuid::new_v4().to_string(), "trial"),
                            BTreeSet::new(),
                        )
                    })
                    .await
                    .unwrap()
                    .data["run_id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            }));
        }
        let mut names = BTreeSet::new();
        for job in jobs {
            assert!(names.insert(job.await.unwrap()));
        }
        assert_eq!(names.len(), 8);
        assert!(!names.contains("trial"));
        assert!(
            catalog
                .write(move |db| catalog::register(
                    db,
                    registration(&uid, "different"),
                    BTreeSet::new()
                ))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn sessions_fence_old_writers_and_terminal_states_do_not_regress() {
        let catalog = Catalog::open(std::path::Path::new("unused"), false).unwrap();
        let uid = Uuid::new_v4().to_string();
        let a = uid.clone();
        catalog
            .write(move |db| catalog::register(db, registration(&a, "trial"), BTreeSet::new()))
            .await
            .unwrap();
        let sid = Uuid::new_v4().to_string();
        let a = uid.clone();
        let b = sid.clone();
        catalog
            .write(move |db| catalog::start_session(db, &a, &json!({"session_id":b})))
            .await
            .unwrap();
        let a = uid.clone();
        assert!(
            catalog
                .write(move |db| catalog::start_session(
                    db,
                    &a,
                    &json!({"session_id":Uuid::new_v4().to_string(),"resume":true})
                ))
                .await
                .is_err()
        );
        let a = uid.clone();
        let b = sid.clone();
        catalog
            .write(move |db| {
                catalog::update_session(
                    db,
                    &a,
                    &b,
                    &json!({"sequence":1,"status":"completed"}),
                    true,
                )
            })
            .await
            .unwrap();
        let a = uid.clone();
        let b = sid.clone();
        assert!(
            catalog
                .write(move |db| catalog::update_session(db, &a, &b, &json!({"sequence":2}), false))
                .await
                .is_err()
        );
        let a = uid.clone();
        let second = Uuid::new_v4().to_string();
        let b = second.clone();
        catalog
            .write(move |db| catalog::start_session(db, &a, &json!({"session_id":b,"resume":true})))
            .await
            .unwrap();
        let a = uid.clone();
        assert!(
            catalog
                .write(move |db| catalog::update_session(
                    db,
                    &a,
                    &sid,
                    &json!({"sequence":3,"status":"failed"}),
                    true
                ))
                .await
                .is_err()
        );
        let current = catalog
            .read(move |db| catalog::run_by_id(db, &uid))
            .await
            .unwrap();
        assert_eq!(current.data["active_session"], second);
        assert_eq!(current.data["attempt"], 2);
        assert_eq!(current.data["status"], "running");
    }

    #[tokio::test]
    async fn catalog_survives_reopen_and_refuses_unknown_schema_versions() {
        let root = std::env::temp_dir().join(format!("aeroboard-catalog-{}", Uuid::new_v4()));
        let catalog = Catalog::open(&root, true).unwrap();
        let identity = catalog.store_id.clone();
        catalog
            .write(|db| {
                catalog::register(
                    db,
                    registration(&Uuid::new_v4().to_string(), "saved"),
                    BTreeSet::new(),
                )
            })
            .await
            .unwrap();
        drop(catalog);
        let catalog = Catalog::open(&root, true).unwrap();
        assert_eq!(catalog.store_id, identity);
        assert_eq!(
            catalog
                .read(|db| catalog::list(db, "run", None))
                .await
                .unwrap()
                .len(),
            1
        );
        catalog
            .write(|db| {
                db.execute_batch("PRAGMA user_version=99")?;
                Ok(())
            })
            .await
            .unwrap();
        drop(catalog);
        assert!(Catalog::open(&root, true).is_err());
    }

    #[tokio::test]
    async fn http_views_require_current_revision_and_stay_with_their_project() {
        let state = state();
        let app = crate::build_app(state);
        let (status, _) = call(
            &app,
            "POST",
            "/api/projects",
            json!({"id":"research","name":"Research"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, created) = call(
            &app,
            "POST",
            "/api/projects/default/views",
            json!({"name":"Overview","settings":{"removedTargets":["loss"]}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let id = created["id"].as_str().unwrap();
        let (status, _) = call(
            &app,
            "PATCH",
            &format!("/api/views/{id}"),
            json!({"revision":0,"name":"stale","settings":{}}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = call(
            &app,
            "PATCH",
            &format!("/api/views/{id}"),
            json!({"revision":1,"name":"Updated","settings":{"starred":["loss"]}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(
            &app,
            "PATCH",
            "/api/projects/research",
            json!({"revision":1,"default_view":id}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn heartbeat_does_not_invalidate_metadata_edits_and_stale_metadata_cannot_overwrite() {
        let state = state();
        let app = crate::build_app(state);
        let uid = Uuid::new_v4().to_string();
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/runs",
                json!({"run_uid":uid,"name":"trial"})
            )
            .await
            .0,
            StatusCode::OK
        );
        let sid = Uuid::new_v4().to_string();
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("/api/runs/{uid}/sessions"),
                json!({"session_id":sid})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("/api/runs/{uid}/sessions/{sid}/heartbeat"),
                json!({"sequence":1})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            call(
                &app,
                "PATCH",
                &format!("/api/run-records/{uid}"),
                json!({"revision":1,"notes":"new"})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            call(
                &app,
                "PATCH",
                &format!("/api/run-records/{uid}"),
                json!({"revision":1,"notes":"old"})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }

    fn point(timestamp: i64, step: Option<f64>, value: f64) -> crate::history::HistoryPoint {
        crate::history::HistoryPoint {
            segment: None,
            ts_ms: timestamp,
            run_id: "run".into(),
            name: "loss".into(),
            step,
            value,
        }
    }
    #[tokio::test]
    async fn legacy_and_identity_registration_reserve_names_together() {
        let app = crate::build_app(state());
        let mut jobs = vec![];
        for index in 0..12 {
            let app = app.clone();
            jobs.push(tokio::spawn(async move {
                let (path, body) = if index % 2 == 0 {
                    (
                        "/api/runs",
                        json!({"run_uid":Uuid::new_v4().to_string(),"name":"mixed"}),
                    )
                } else {
                    ("/api/runs/resolve", json!({"base_run_id":"mixed"}))
                };
                let (status, body) = call(&app, "POST", path, body).await;
                assert_eq!(status, StatusCode::OK);
                if index % 2 == 0 {
                    body["data"]["run_id"].as_str().unwrap().to_owned()
                } else {
                    body["run_id"].as_str().unwrap().to_owned()
                }
            }));
        }
        let mut names = BTreeSet::new();
        for job in jobs {
            assert!(names.insert(job.await.unwrap()));
        }
        assert_eq!(names.len(), 12);
    }
    #[test]
    fn bounded_history_keeps_lookahead_for_pagination_after_late_replay() {
        let query = crate::storage::HistoryQueryOptions {
            limit: Some(2),
            cursor: Some(1),
            ..Default::default()
        };
        let mut selection = crate::storage::query::HistorySelection::new(&query);
        for time in [10, 3, 8, 1, 5] {
            selection.push(point(time, None, time as f64));
        }
        let page = crate::storage::query::apply_history_query(selection.finish(), &query);
        assert_eq!(
            page.rows.iter().map(|p| p.ts_ms).collect::<Vec<_>>(),
            vec![3, 5]
        );
        assert_eq!(page.next_cursor, Some(3));
    }
    #[test]
    fn summary_ranges_ignore_all_nonfinite_values_and_signed_zero_ties_are_earliest() {
        let rows = vec![
            point(1, None, f64::NEG_INFINITY),
            point(2, None, 3.0),
            point(3, None, f64::INFINITY),
            point(4, None, 2.0),
            point(5, None, f64::NAN),
        ];
        let summaries = crate::history::build_run_summaries(&rows, vec![]);
        assert_eq!(summaries[0].metric_summaries[0].min, 2.0);
        assert_eq!(summaries[0].metric_summaries[0].max, 3.0);
        let score = crate::analytics::calculate(
            "run",
            "loss",
            vec![point(1, Some(1.0), 0.0), point(2, Some(2.0), -0.0)],
            &json!({"goal":"minimize"}),
            None,
        );
        assert_eq!(score.best_step, Some(1.0));
    }
    #[test]
    fn scores_handle_zero_targets_nonfinite_data_ties_and_step_coverage() {
        let rows = vec![
            point(100, Some(1.0), 1.0),
            point(300, Some(3.0), 0.0),
            point(200, Some(2.0), 0.0),
            point(400, Some(4.0), f64::NAN),
        ];
        let score = crate::analytics::calculate(
            "run",
            "loss",
            rows.clone(),
            &json!({"goal":"minimize","target":0}),
            Some(2.5),
        );
        assert_eq!(score.best, Some(0.0));
        assert_eq!(score.best_step, Some(2.0));
        assert_eq!(score.time_to_target_ms, Some(100));
        assert_eq!(score.at_step, Some(0.0));
        assert_eq!(score.last, None);
        assert_eq!(score.finite_count, 3);
        assert_eq!(
            crate::analytics::calculate("run", "loss", rows.clone(), &json!({}), Some(5.0)).at_step,
            None
        );
        assert_eq!(
            crate::analytics::calculate("run", "loss", rows, &json!({}), None).best,
            None
        );
    }
}
