use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::test_support::{
    Built, Env, Pkg, apply, assert_consistent, assert_mode, commit, gen_input, name, plan,
    view_input,
};
use crate::{Error, Transaction, fault};

fn vi_pair(env: &Env, group: bool) -> (Built, Built) {
    let mut neovim = Pkg::new("neovim", "0.11.4").file("usr/bin/vi", b"neovim");
    let mut vim = Pkg::new("vim", "9.1").file("usr/bin/vi", b"vim");
    if group {
        neovim = neovim.alternative("vi", &["usr/bin/vi"]);
        vim = vim.alternative("vi", &["usr/bin/vi"]);
    }
    (neovim.build(env), vim.build(env))
}

/// Adds `packages` and tries to assemble a generation with `choices`.
fn assemble(env: &Env, packages: &[&Built], choices: &[(&str, &str)]) -> crate::Result<u64> {
    let lock = env.lock();
    let mut tx = Transaction::begin(&lock, &plan(&env.root, &[]))?;
    for package in packages {
        tx.add(&package.path, &package.id)?;
    }
    let mut input = gen_input(packages, &[]);
    input.alternatives = choices.iter().map(|(g, c)| (name(g), name(c))).collect();
    tx.assemble_generation(&input)?;
    tx.make_default()
}

fn same_inode(a: &Path, b: &Path) -> bool {
    let (a, b) = (
        fs::metadata(a).expect("exists"),
        fs::metadata(b).expect("exists"),
    );
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

#[test]
fn generation_tree_links_every_store_file() {
    let env = Env::new();
    let libc = Pkg::new("libc", "1.2")
        .file("usr/lib/libc.so.6", b"libc")
        .symlink("usr/lib/libc.so", "libc.so.6")
        .build(&env);
    let zlib = Pkg::new("zlib", "1.3")
        .file("usr/lib/libz.so.1", b"zlib")
        .build(&env);
    let tool = Pkg::new("tool", "2")
        .file("usr/bin/tool", b"tool")
        .file("etc/tool.conf", b"conf")
        .build(&env);
    let number = commit(&env, &[&libc, &zlib, &tool]);
    assert_eq!(number, 1);

    let usr = env.root.gen_dir(1).join("usr");
    let store = env.root.store();
    for (package, path) in [
        (&libc, "usr/lib/libc.so.6"),
        (&zlib, "usr/lib/libz.so.1"),
        (&tool, "usr/bin/tool"),
    ] {
        assert!(same_inode(
            &env.root.gen_dir(1).join(path),
            &store.path(&package.dir).join(path)
        ));
    }
    assert_eq!(
        fs::read_link(usr.join("lib/libc.so")).expect("a symlink"),
        Path::new("libc.so.6")
    );
    assert!(!env.root.gen_dir(1).join("etc/tool.conf").exists());
    assert_mode(&usr, 0o555);
    assert_mode(&usr.join("lib"), 0o555);
    assert_consistent(&env.root);
}

#[test]
fn generation_records_its_inputs() {
    let env = Env::new();
    let tool = Pkg::new("tool", "2")
        .file("usr/bin/tool", b"tool")
        .build(&env);
    let first = commit(&env, &[&tool]);
    let second = commit(&env, &[&tool]);
    assert_eq!((first, second), (1, 2));
    assert_eq!(env.root.default_generation().expect("readable"), Some(2));

    let generation = env.root.generation(2).expect("complete");
    let input = gen_input(&[&tool], &[]);
    assert_eq!(generation.parent, Some(1));
    assert_eq!(
        generation.config.hash,
        laft_core::Blake3Hash::of(&input.config)
    );
    assert_eq!(generation.init, input.init);
    assert_eq!(generation.message.as_deref(), Some("test"));
    assert_eq!(generation.packages.len(), 1);
    assert_eq!(generation.packages[0].id, tool.id);
    assert_eq!(generation.packages[0].dir, tool.dir);
    assert!(generation.initrd_packages.is_empty());
    let dir = env.root.gen_dir(2);
    assert_eq!(
        fs::read(dir.join("config.json")).expect("readable"),
        input.config
    );
    assert_mode(&dir.join("gen.json"), 0o444);
    for empty in ["etc", "services", "boot"] {
        assert_eq!(fs::read_dir(dir.join(empty)).expect("exists").count(), 0);
    }
}

#[test]
fn conflicting_paths_name_both_packages() {
    let env = Env::new();
    let (neovim, vim) = vi_pair(&env, false);
    let error = assemble(&env, &[&neovim, &vim], &[]).expect_err("both own usr/bin/vi");
    assert!(matches!(error, Error::Conflicts { .. }));
    assert_eq!(
        error.to_string(),
        "view \"system\": 1 path conflict\n  usr/bin/vi\n    neovim 0.11.4-r1\n    vim 9.1-r1\n  \
         hint: these packages do not declare a common alternatives group, so remove one of them"
    );
    assert_eq!(env.root.generations().expect("lists"), Vec::<u64>::new());
    env.lock().recover().expect("recovers");
    assert_consistent(&env.root);
}

#[test]
fn chosen_alternative_provides_the_shared_path() {
    let env = Env::new();
    let (neovim, vim) = vi_pair(&env, true);
    assemble(&env, &[&neovim, &vim], &[("vi", "neovim")]).expect("the choice resolves it");
    let generation = env.root.generation(1).expect("complete");
    assert_eq!(
        generation.alternatives,
        BTreeMap::from([(name("vi"), name("neovim"))])
    );
    assert!(same_inode(
        &env.root.gen_dir(1).join("usr/bin/vi"),
        &env.root.store().path(&neovim.dir).join("usr/bin/vi")
    ));
    assert_consistent(&env.root);
}

#[test]
fn absent_alternative_choice_is_rejected() {
    let env = Env::new();
    let gcc = Pkg::new("gcc", "15")
        .file("usr/bin/cc", b"gcc")
        .alternative("cc", &["usr/bin/cc"])
        .build(&env);
    let error = assemble(&env, &[&gcc], &[("cc", "clang")]).expect_err("clang is absent");
    assert_eq!(
        error.to_string(),
        "alternatives.cc = clang, but clang is not in view \"system\"\n  members present: gcc"
    );
}

#[test]
fn several_alternatives_without_a_choice_are_rejected() {
    let env = Env::new();
    let (neovim, vim) = vi_pair(&env, true);
    let error = assemble(&env, &[&neovim, &vim], &[]).expect_err("no choice");
    assert_eq!(
        error.to_string(),
        "alternatives group \"vi\" has several members in view \"system\": neovim, vim\n  \
         hint: choose one with `alternatives.vi = neovim`"
    );
}

#[test]
fn two_versions_of_one_package_are_rejected() {
    let env = Env::new();
    let old = Pkg::new("tool", "1").file("usr/bin/tool", b"1").build(&env);
    let new = Pkg::new("tool", "2").file("usr/bin/tool", b"2").build(&env);
    let error = assemble(&env, &[&old, &new], &[]).expect_err("tool twice");
    assert!(matches!(error, Error::DuplicatePackage { name, .. } if name.as_str() == "tool"));
}

#[test]
fn link_headroom_stops_assembly_before_creating_anything() {
    let env = Env::new();
    let tool = Pkg::new("tool", "2")
        .file("usr/bin/tool", b"tool")
        .build(&env);
    fault::set_link_max(Some(20));
    for expected in 1..=3 {
        assert_eq!(commit(&env, &[&tool]), expected);
    }
    let store_file = env.root.store().path(&tool.dir).join("usr/bin/tool");
    assert_eq!(fs::metadata(&store_file).expect("exists").nlink(), 4);
    let lock = env.lock();
    let error = apply(&lock, &[&tool], None).expect_err("the store file has 4 links");
    fault::set_link_max(None);
    assert!(
        matches!(&error, Error::LinkLimit { path, links: 4, limit: 20 } if *path == store_file),
        "{error}"
    );
    assert_eq!(fs::read_dir(env.root.tmp_dir()).expect("exists").count(), 0);
    assert_eq!(env.root.generations().expect("lists"), vec![1, 2, 3]);
    assert_eq!(fs::metadata(&store_file).expect("exists").nlink(), 4);
}

#[test]
fn declared_view_records_packages_and_access() {
    let env = Env::new();
    let tool = Pkg::new("tool", "2")
        .file("usr/bin/tool", b"tool")
        .build(&env);
    let lock = env.lock();
    apply(&lock, &[&tool], Some("dev")).expect("succeeds");
    let view = env.root.view(&name("dev"), 1).expect("complete");
    assert_eq!(view.packages.len(), 1);
    assert_eq!(view.users, vec!["qaid".to_owned()]);
    assert_eq!(view.allowed_uids, vec![1000]);
    assert_eq!(view.groups, vec!["wheel".to_owned()]);
    assert_eq!(view.allowed_gids, vec![10]);
    assert!(same_inode(
        &env.root.view_dir(&name("dev"), 1).join("usr/bin/tool"),
        &env.root.store().path(&tool.dir).join("usr/bin/tool")
    ));
    let generation = env.root.generation(1).expect("complete");
    assert_eq!(generation.views, BTreeMap::from([(name("dev"), 1)]));
    apply(&lock, &[&tool], Some("dev")).expect("succeeds");
    assert_eq!(
        env.root.view_versions(&name("dev")).expect("lists"),
        vec![1, 2]
    );
    assert_consistent(&env.root);
}

#[test]
fn a_declared_view_cannot_be_named_system() {
    let env = Env::new();
    let lock = env.lock();
    let error = Transaction::begin(&lock, &plan(&env.root, &["system"])).expect_err("reserved");
    assert!(matches!(error, Error::Invalid(_)));
}

#[test]
fn view_inputs_must_be_planned() {
    let env = Env::new();
    let tool = Pkg::new("tool", "2")
        .file("usr/bin/tool", b"tool")
        .build(&env);
    let lock = env.lock();
    let mut tx = Transaction::begin(&lock, &plan(&env.root, &[])).expect("begins");
    tx.add(&tool.path, &tool.id).expect("added");
    let error = tx
        .assemble_view(&view_input("dev", &[&tool]))
        .expect_err("not planned");
    assert!(matches!(error, Error::Invalid(_)));
}
