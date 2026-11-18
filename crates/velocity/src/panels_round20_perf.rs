use super::*;

#[test]
#[ignore = "100k-file timing fixture"]
fn r20_project_walk_and_sidebar_refresh_100k() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    for n in 0..100_000 {
        std::fs::write(root.join(format!("file{n:06}.rs")), b"fn example() {}\n").unwrap();
    }
    let folder = Folder {
        path: root.clone(),
        name: "fixture".into(),
        follow_links: true,
        rules: Rules::new(
            &["*.tmp".into()],
            &[".git".into(), "vendor".into()],
            &["*.png".into()],
        )
        .unwrap(),
    };
    let wait = |mut ready: Box<dyn FnMut() -> bool + '_>| {
        let start = Instant::now();
        while !ready() {
            assert!(
                start.elapsed() < Duration::from_secs(45),
                "benchmark did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    for _ in 0..5 {
        let start = Instant::now();
        let index = Index::new(vec![folder.clone()]);
        wait(Box::new(|| {
            index
                .poll()
                .is_some_and(|snapshot| snapshot.files.len() == 100_000)
        }));
        println!("cold_walk_ms={:.3}", start.elapsed().as_secs_f64() * 1000.);
        drop(index);
    }
    let mut workspace = Workspace::new(vel_text::Text::new(), None, None, false).unwrap();
    workspace.settings.set(
        vel_config::settings::Layer::User,
        "index_files",
        json!(false),
    );
    let mut panels = Panels::new(&workspace, false);
    panels.add_folder(&mut workspace, root.clone()).unwrap();
    wait(Box::new(|| {
        panels.poll(&mut workspace, 0..0);
        panels.side_rows.len() == 100_001
    }));
    for n in 0..5 {
        let path = root.join(format!("zz{n}.txt"));
        std::fs::write(&path, "refresh").unwrap();
        let start = Instant::now();
        panels.refresh_sidebar(&mut workspace);
        let dispatch = start.elapsed();
        wait(Box::new(|| {
            panels.poll(&mut workspace, 0..0);
            panels.side_rows.last().is_some_and(|row| row.path == path)
        }));
        println!(
            "sidebar_dispatch_ms={:.3} sidebar_ready_ms={:.3}",
            dispatch.as_secs_f64() * 1000.,
            start.elapsed().as_secs_f64() * 1000.
        );
    }
}
