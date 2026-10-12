use super::{generated as queries, optional};
use sqlx::PgPool;

#[sqlx::test(migrator = "super::MIGRATOR")]
#[ignore = "requires PostgreSQL"]
async fn optional_sessions_preserve_missing_expired_and_database_errors(pool: PgPool) {
    assert!(
        optional(queries::get_session(&pool, "missing").await)
            .unwrap()
            .is_none()
    );
    queries::create_session(
        &pool,
        queries::CreateSessionParams {
            token_hash: "session",
            csrf_token: "csrf",
        },
    )
    .await
    .unwrap();
    assert_eq!(
        queries::get_session(&pool, "session")
            .await
            .unwrap()
            .csrf_token,
        "csrf"
    );
    sqlx::query("UPDATE web_sessions SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        optional(queries::get_session(&pool, "session").await)
            .unwrap()
            .is_none()
    );
    queries::delete_expired_sessions(&pool).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM web_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    pool.close().await;
    assert!(matches!(
        optional(queries::get_session(&pool, "session").await),
        Err(sqlx::Error::PoolClosed)
    ));
}

#[sqlx::test(migrator = "super::MIGRATOR")]
#[ignore = "requires PostgreSQL"]
async fn recursive_parent_check_detects_indirect_cycles(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    let mut posts = Vec::new();
    for hash in ["one", "two", "three"] {
        let asset_id = queries::upsert_media_asset(
            &mut tx,
            queries::UpsertMediaAssetParams {
                sha_256: hash,
                md_5: hash,
                file_ext: "png",
                content_type: "image/png",
                file_size: 1,
                image_width: 1,
                image_height: 1,
                duration: None,
                original_key: hash,
            },
        )
        .await
        .unwrap()
        .id;
        posts.push(
            queries::create_post(
                &mut tx,
                queries::CreatePostParams {
                    media_asset_id: asset_id,
                    source: "",
                    rating: "g",
                },
            )
            .await
            .unwrap()
            .id,
        );
    }
    for pair in posts.windows(2) {
        queries::set_post_parent(
            &mut tx,
            queries::SetPostParentParams {
                id: pair[1],
                parent_id: Some(pair[0]),
            },
        )
        .await
        .unwrap();
    }
    assert!(
        queries::would_create_parent_cycle(
            &mut tx,
            queries::WouldCreateParentCycleParams {
                id: posts[2],
                post_id: posts[0]
            }
        )
        .await
        .unwrap()
        .cycle
    );
    assert!(
        !queries::would_create_parent_cycle(
            &mut tx,
            queries::WouldCreateParentCycleParams {
                id: posts[0],
                post_id: posts[2]
            }
        )
        .await
        .unwrap()
        .cycle
    );
}
