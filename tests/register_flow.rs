use std::fs;

use daihon::model::{Priority, validate_path};
use daihon::register::{propose_registration, save_registration};
use daihon::runner::mock::MockRunner;
use serde_json::json;
use tempfile::TempDir;

#[test]
fn register_creates_valid_scenario_detects_overlap_and_never_modifies_existing_files() {
    let dir = TempDir::new().unwrap();
    let scenarios_dir = dir.path().join("scenarios");
    fs::create_dir_all(&scenarios_dir).unwrap();

    // Seed an existing scenario file and snapshot its exact bytes.
    let existing_path = scenarios_dir.join("01-rio-herdr-cross-machine.md");
    let existing_bytes = fs::read("examples/scenarios/01-rio-herdr-cross-machine.md").unwrap();
    fs::write(&existing_path, &existing_bytes).unwrap();

    let mock_runner = MockRunner::with_json_responses(
        "mock-register",
        vec![
            json!({
                "id": "ubuntu-gpu-build-from-rio",
                "title": "Rio から Ubuntu の RTX 4090 上で学習ジョブを走らせる",
                "priority": "P1",
                "priority_rationale": "既存のマルチマシン中核ワークフロー（P0）を拡張する重要ユースケースのため P1",
                "tags": ["multi-machine", "ubuntu", "gpu"],
                "overview": "MacBook Air の Rio から Ubuntu 側の GPU ジョブを起動し、ログを見ながら開発を続ける。",
                "steps": [
                    {
                        "title": "Ubuntu 側の GPU 状態を確認できる",
                        "checks": [
                            { "id": "s1-c1", "mode": "auto", "description": "SSH 経由で nvidia-smi が終了コード 0 で返る" }
                        ]
                    },
                    {
                        "title": "Rio のペイン上で学習ログを監視できる",
                        "checks": [
                            { "id": "s2-c1", "mode": "auto", "description": "ジョブ起動コマンドがジョブ ID を出力する" },
                            { "id": "s2-c2", "mode": "manual", "description": "Rio 上でログの追従表示が崩れない" }
                        ]
                    }
                ],
                "overlaps": [
                    {
                        "existing_id": "rio-herdr-cross-machine",
                        "reason": "MacBook Air の Rio から Ubuntu へ接続して作業を継続する前提部分が重なる"
                    }
                ]
            }),
            json!({
                "id": "offline-notes-sync",
                "title": "オフライン時の作業メモを再接続後に同期できる",
                "priority": "P2",
                "priority_rationale": "補助的な利便性機能のため P2",
                "tags": ["sync", "offline"],
                "overview": "移動中にオフラインで残した作業ログを、ネットワーク復帰後に自動で統合できる。",
                "steps": [
                    {
                        "title": "ローカルへ一時保存し再接続で同期する",
                        "checks": [
                            { "id": "s1-c1", "mode": "auto", "description": "同期コマンドが未送信ログをエラーなく反映する" }
                        ]
                    }
                ],
                "overlaps": []
            }),
        ],
    );

    // 1. Register overlapping scenario.
    let proposal1 = propose_registration(
        "Rio から Ubuntu の RTX 4090 で学習ジョブを回したい",
        &scenarios_dir,
        &mock_runner,
        dir.path(),
        60,
    )
    .expect("proposal 1 should succeed");

    assert_eq!(proposal1.priority_proposal.suggested, Priority::P1);
    assert!(
        !proposal1.overlaps.is_empty(),
        "既存シナリオとの重なりが検出されるべき"
    );
    assert_eq!(proposal1.overlaps[0].existing_id, "rio-herdr-cross-machine");

    let saved1 = save_registration(&proposal1, &scenarios_dir, None).expect("save 1");
    assert_eq!(
        saved1.file_name().unwrap().to_str().unwrap(),
        "02-ubuntu-gpu-build-from-rio.md"
    );
    let saved1_bytes = fs::read(&saved1).unwrap();

    // 2. Register a second scenario sequentially (overriding priority to P1).
    let proposal2 = propose_registration(
        "オフラインで書いたメモを復帰後に同期したい",
        &scenarios_dir,
        &mock_runner,
        dir.path(),
        60,
    )
    .expect("proposal 2 should succeed");

    let saved2 = save_registration(&proposal2, &scenarios_dir, Some(Priority::P1)).expect("save 2");
    assert_eq!(
        saved2.file_name().unwrap().to_str().unwrap(),
        "03-offline-notes-sync.md"
    );

    // 3. Existing files must remain byte-for-byte unchanged, and all 3 pass format validation!
    assert_eq!(
        fs::read(&existing_path).unwrap(),
        existing_bytes,
        "1 本目の既存シナリオファイルが変わってはならない"
    );
    assert_eq!(
        fs::read(&saved1).unwrap(),
        saved1_bytes,
        "2 本目の既存シナリオファイルも追加登録で変わってはならない"
    );

    let report = validate_path(&scenarios_dir);
    assert!(report.is_ok(), "全 3 ファイルが形式チェックを通るべき: {report:#?}");
    assert_eq!(report.files.len(), 3);
}
