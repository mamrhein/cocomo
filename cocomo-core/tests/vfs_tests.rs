// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{io, path::Path, path::PathBuf};

use cocomo_core::vfs::{FSItemKind, InMemoryFS, Vfs, VfsError, VfsImpl};
use cocomo_core::{VfsBackend, VfsItem};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup_fs() -> VfsImpl<InMemoryFS> {
    let imfs = InMemoryFS::new();
    imfs.create_dir_all(Path::new("/dir_a")).await.unwrap();
    imfs.create_file(Path::new("/dir_a/nested.txt"), b"nested content");
    imfs.create_file(Path::new("/file_a.txt"), b"hello world");
    imfs.create_symlink(Path::new("/link_a"), Path::new("/file_a.txt"));
    imfs.create_symlink(Path::new("/link_broken"), Path::new("/nonexistent"));
    VfsImpl::new(imfs)
}

async fn setup_deep_symlinks() -> VfsImpl<InMemoryFS> {
    let imfs = InMemoryFS::new();
    imfs.create_file(Path::new("/target"), b"found");
    let mut prev = PathBuf::from("/a00");
    for i in 1..=33 {
        let cur = PathBuf::from(format!("/a{:02}", i));
        imfs.create_symlink(&prev, &cur);
        prev = cur;
    }
    imfs.create_symlink(&prev, Path::new("/target"));
    VfsImpl::new(imfs)
}

// ---------------------------------------------------------------------------
// new_item
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_new_item_file() {
    let vfs = setup_fs().await;
    let item = vfs.new_item(Path::new("/file_a.txt")).await;
    assert!(item.is_file());
    assert_eq!(item.name, "file_a.txt");
    assert_eq!(item.path, Path::new("/file_a.txt"));
}

#[tokio::test]
async fn test_new_item_dir() {
    let vfs = setup_fs().await;
    let item = vfs.new_item(Path::new("/dir_a")).await;
    assert!(item.is_dir());
    assert_eq!(item.name, "dir_a");
}

#[tokio::test]
async fn test_new_item_symlink() {
    let vfs = setup_fs().await;
    let item = vfs.new_item(Path::new("/link_a")).await;
    assert!(item.is_link());
    assert_eq!(item.link_target(), Some(Path::new("/file_a.txt")));
}

#[tokio::test]
async fn test_new_item_broken_symlink() {
    let vfs = setup_fs().await;
    let item = vfs.new_item(Path::new("/link_broken")).await;
    assert!(item.is_link());
    assert_eq!(item.link_target(), Some(Path::new("/nonexistent")));
}

#[tokio::test]
async fn test_new_item_non_existent() {
    let vfs = setup_fs().await;
    let item = vfs.new_item(Path::new("/nonexistent")).await;
    assert!(matches!(
        item.kind,
        FSItemKind::Invalid {
            cause: io::ErrorKind::NotFound
        }
    ));
}

// ---------------------------------------------------------------------------
// read_dir
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_dir_root() {
    let vfs = setup_fs().await;
    let root = vfs.new_item(Path::new("/")).await;
    let entries = vfs.read_dir(&root).await.unwrap();
    let mut names: Vec<&str> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, vec!["dir_a", "file_a.txt", "link_a", "link_broken"]);
}

#[tokio::test]
async fn test_read_dir_nested() {
    let vfs = setup_fs().await;
    let dir_a = vfs.new_item(Path::new("/dir_a")).await;
    let entries = vfs.read_dir(&dir_a).await.unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
    assert_eq!(names, vec!["nested.txt"]);
}

#[tokio::test]
async fn test_read_dir_empty() {
    let imfs = InMemoryFS::new();
    imfs.create_dir_all(Path::new("/empty")).await.unwrap();
    let vfs = VfsImpl::new(imfs);
    let dir = vfs.new_item(Path::new("/empty")).await;
    let entries = vfs.read_dir(&dir).await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn test_read_dir_on_file() {
    let vfs = setup_fs().await;
    let file = vfs.new_item(Path::new("/file_a.txt")).await;
    let err = vfs.read_dir(&file).await.unwrap_err();
    assert!(matches!(err, VfsError::NotADirectory(_)));
}

#[tokio::test]
async fn test_read_dir_non_existent() {
    let vfs = setup_fs().await;
    let invalid = vfs.new_item(Path::new("/nonexistent")).await;
    let err = vfs.read_dir(&invalid).await.unwrap_err();
    assert!(matches!(err, VfsError::NotADirectory(_)));
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_file() {
    let vfs = setup_fs().await;
    let file = vfs.new_item(Path::new("/file_a.txt")).await;
    let content = vfs.read_file(&file).await.unwrap();
    assert_eq!(content, "hello world");
}

#[tokio::test]
async fn test_read_file_nested() {
    let vfs = setup_fs().await;
    let file = vfs.new_item(Path::new("/dir_a/nested.txt")).await;
    let content = vfs.read_file(&file).await.unwrap();
    assert_eq!(content, "nested content");
}

#[tokio::test]
async fn test_read_file_on_dir() {
    let vfs = setup_fs().await;
    let dir = vfs.new_item(Path::new("/dir_a")).await;
    let err = vfs.read_file(&dir).await.unwrap_err();
    assert!(matches!(err, VfsError::Unsupported(_)));
}

#[tokio::test]
async fn test_read_file_invalid_item() {
    let vfs = setup_fs().await;
    let invalid = vfs.new_item(Path::new("/nonexistent")).await;
    let err = vfs.read_file(&invalid).await.unwrap_err();
    assert!(matches!(err, VfsError::Unsupported(_)));
}

#[tokio::test]
async fn test_read_file_deleted() {
    let vfs = setup_fs().await;
    let file = vfs.new_item(Path::new("/file_a.txt")).await;
    vfs.delete(&file).await.unwrap();
    let err = vfs.read_file(&file).await.unwrap_err();
    assert!(matches!(
        err,
        VfsError::Io(ref e) if e.kind() == io::ErrorKind::NotFound
    ));
}

// ---------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_resolve_symlink() {
    let vfs = setup_fs().await;
    let link = vfs.new_item(Path::new("/link_a")).await;
    let resolved = vfs.resolve(&link).await.unwrap();
    assert!(resolved.is_file());
    assert_eq!(resolved.path, Path::new("/file_a.txt"));
}

#[tokio::test]
async fn test_resolve_non_symlink() {
    let vfs = setup_fs().await;
    let dir = vfs.new_item(Path::new("/dir_a")).await;
    let resolved = vfs.resolve(&dir).await.unwrap();
    assert!(resolved.is_dir());
    assert_eq!(resolved.path, Path::new("/dir_a"));
}

#[tokio::test]
async fn test_resolve_broken_symlink() {
    let vfs = setup_fs().await;
    let link = vfs.new_item(Path::new("/link_broken")).await;
    let resolved = vfs.resolve(&link).await.unwrap();
    assert!(matches!(resolved.kind, FSItemKind::Invalid { .. }));
}

#[tokio::test]
async fn test_resolve_symlink_chain_max_depth() {
    let vfs = setup_deep_symlinks().await;
    let start = vfs.new_item(Path::new("/a00")).await;
    let resolved = vfs.resolve(&start).await.unwrap();
    assert!(resolved.is_link());
}

#[tokio::test]
async fn test_resolve_self_referential_symlink() {
    let imfs = InMemoryFS::new();
    imfs.create_dir_all(Path::new("/loop")).await.unwrap();
    imfs.create_symlink(Path::new("/loop/self"), Path::new("/loop/self"));
    let vfs = VfsImpl::new(imfs);
    let loop_item = vfs.new_item(Path::new("/loop/self")).await;
    let resolved = vfs.resolve(&loop_item).await.unwrap();
    assert!(resolved.is_link());
}

// ---------------------------------------------------------------------------
// comparable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_comparable_dirs() {
    let vfs = setup_fs().await;
    let a = vfs.new_item(Path::new("/dir_a")).await;
    let b = vfs.new_item(Path::new("/")).await;
    assert!(vfs.comparable(&a, &b).await);
}

#[tokio::test]
async fn test_comparable_files() {
    let vfs = setup_fs().await;
    let a = vfs.new_item(Path::new("/file_a.txt")).await;
    let b = vfs.new_item(Path::new("/dir_a/nested.txt")).await;
    assert!(vfs.comparable(&a, &b).await);
}

#[tokio::test]
async fn test_comparable_dir_file() {
    let vfs = setup_fs().await;
    let dir = vfs.new_item(Path::new("/dir_a")).await;
    let file = vfs.new_item(Path::new("/file_a.txt")).await;
    assert!(!vfs.comparable(&dir, &file).await);
}

#[tokio::test]
async fn test_comparable_file_symlink() {
    let vfs = setup_fs().await;
    let file = vfs.new_item(Path::new("/file_a.txt")).await;
    let link = vfs.new_item(Path::new("/link_a")).await;
    assert!(!vfs.comparable(&file, &link).await);
}

#[tokio::test]
async fn test_comparable_dir_symlink() {
    let vfs = setup_fs().await;
    let dir = vfs.new_item(Path::new("/")).await;
    let link = vfs.new_item(Path::new("/link_a")).await;
    assert!(!vfs.comparable(&dir, &link).await);
}

// ---------------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_copy_file() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/file_a.txt")).await;
    let dst = vfs.new_item(Path::new("/copy_of_a.txt")).await;
    vfs.copy(&src, &dst).await.unwrap();
    let copied = vfs.new_item(Path::new("/copy_of_a.txt")).await;
    assert!(copied.is_file());
    assert_eq!(vfs.read_file(&copied).await.unwrap(), "hello world");
}

#[tokio::test]
async fn test_copy_file_into_dir() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/file_a.txt")).await;
    let dst_dir = vfs.new_item(Path::new("/dir_a")).await;
    vfs.copy(&src, &dst_dir).await.unwrap();
    let copied = vfs.new_item(Path::new("/dir_a/file_a.txt")).await;
    assert!(copied.is_file());
    assert_eq!(vfs.read_file(&copied).await.unwrap(), "hello world");
}

#[tokio::test]
async fn test_copy_dir() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/dir_a")).await;
    let dst = vfs.new_item(Path::new("/dir_b")).await;
    vfs.copy(&src, &dst).await.unwrap();
    let copied_dir = vfs.new_item(Path::new("/dir_b")).await;
    assert!(copied_dir.is_dir());
    let copied_file = vfs.new_item(Path::new("/dir_b/nested.txt")).await;
    assert!(copied_file.is_file());
    assert_eq!(
        vfs.read_file(&copied_file).await.unwrap(),
        "nested content"
    );
}

#[tokio::test]
async fn test_copy_dir_into_dir() {
    let vfs = setup_fs().await;
    vfs.backend.create_dir_all(Path::new("/dir_target")).await.unwrap();
    let src = vfs.new_item(Path::new("/dir_a")).await;
    let dst = vfs.new_item(Path::new("/dir_target")).await;
    vfs.copy(&src, &dst).await.unwrap();
    let copied = vfs.new_item(Path::new("/dir_target/dir_a")).await;
    assert!(copied.is_dir());
    let nested = vfs.new_item(Path::new("/dir_target/dir_a/nested.txt")).await;
    assert!(nested.is_file());
}

#[tokio::test]
async fn test_copy_symlink() {
    let vfs = setup_fs().await;
    let link = vfs.new_item(Path::new("/link_a")).await;
    let dst = vfs.new_item(Path::new("/dest")).await;
    let err = vfs.copy(&link, &dst).await.unwrap_err();
    assert!(matches!(err, VfsError::Unsupported(_)));
}

#[tokio::test]
async fn test_copy_non_existent() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/nonexistent")).await;
    let dst = vfs.new_item(Path::new("/dest")).await;
    let err = vfs.copy(&src, &dst).await.unwrap_err();
    assert!(matches!(err, VfsError::Unsupported(_)));
}

#[tokio::test]
async fn test_copy_deleted_file() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/file_a.txt")).await;
    let dst = vfs.new_item(Path::new("/copy.txt")).await;
    vfs.delete(&src).await.unwrap();
    let err = vfs.copy(&src, &dst).await.unwrap_err();
    assert!(matches!(
        err,
        VfsError::Io(ref e) if e.kind() == io::ErrorKind::NotFound
    ));
}

// ---------------------------------------------------------------------------
// move_item
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_move_file() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/file_a.txt")).await;
    let dst = vfs.new_item(Path::new("/moved.txt")).await;
    vfs.move_item(&src, &dst).await.unwrap();
    let gone = vfs.new_item(Path::new("/file_a.txt")).await;
    assert!(matches!(gone.kind, FSItemKind::Invalid { .. }));
    let moved = vfs.new_item(Path::new("/moved.txt")).await;
    assert!(moved.is_file());
    assert_eq!(vfs.read_file(&moved).await.unwrap(), "hello world");
}

#[tokio::test]
async fn test_move_file_into_dir() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/file_a.txt")).await;
    let dst_dir = vfs.new_item(Path::new("/dir_a")).await;
    vfs.move_item(&src, &dst_dir).await.unwrap();
    let moved = vfs.new_item(Path::new("/dir_a/file_a.txt")).await;
    assert!(moved.is_file());
    assert_eq!(vfs.read_file(&moved).await.unwrap(), "hello world");
    let gone = vfs.new_item(Path::new("/file_a.txt")).await;
    assert!(matches!(gone.kind, FSItemKind::Invalid { .. }));
}

#[tokio::test]
async fn test_move_dir() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/dir_a")).await;
    let dst = vfs.new_item(Path::new("/dir_moved")).await;
    vfs.move_item(&src, &dst).await.unwrap();
    let moved = vfs.new_item(Path::new("/dir_moved")).await;
    assert!(moved.is_dir());
    let nested = vfs.new_item(Path::new("/dir_moved/nested.txt")).await;
    assert!(nested.is_file());
    let gone = vfs.new_item(Path::new("/dir_a")).await;
    assert!(matches!(gone.kind, FSItemKind::Invalid { .. }));
}

#[tokio::test]
async fn test_move_non_existent() {
    let vfs = setup_fs().await;
    let src = vfs.new_item(Path::new("/nonexistent")).await;
    let dst = vfs.new_item(Path::new("/dest")).await;
    // Invalid item → move_item falls through to backend rename which
    // dispatches on FSItem kind. For Invalid, rename gets called with
    // the path, backend returns NotFound.
    let err = vfs.move_item(&src, &dst).await.unwrap_err();
    assert!(matches!(
        err,
        VfsError::Io(ref e) if e.kind() == io::ErrorKind::NotFound
    ));
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_file() {
    let vfs = setup_fs().await;
    let file = vfs.new_item(Path::new("/file_a.txt")).await;
    vfs.delete(&file).await.unwrap();
    let gone = vfs.new_item(Path::new("/file_a.txt")).await;
    assert!(matches!(gone.kind, FSItemKind::Invalid { .. }));
}

#[tokio::test]
async fn test_delete_dir() {
    let vfs = setup_fs().await;
    let dir = vfs.new_item(Path::new("/dir_a")).await;
    vfs.delete(&dir).await.unwrap();
    let gone = vfs.new_item(Path::new("/dir_a")).await;
    assert!(matches!(gone.kind, FSItemKind::Invalid { .. }));
    let nested_gone = vfs.new_item(Path::new("/dir_a/nested.txt")).await;
    assert!(matches!(nested_gone.kind, FSItemKind::Invalid { .. }));
}

#[tokio::test]
async fn test_delete_file_twice() {
    let vfs = setup_fs().await;
    let file = vfs.new_item(Path::new("/file_a.txt")).await;
    vfs.delete(&file).await.unwrap();
    let err = vfs.delete(&file).await.unwrap_err();
    assert!(matches!(
        err,
        VfsError::Io(ref e) if e.kind() == io::ErrorKind::NotFound
    ));
}

#[tokio::test]
async fn test_delete_invalid_item() {
    let vfs = setup_fs().await;
    let invalid = vfs.new_item(Path::new("/nonexistent")).await;
    let err = vfs.delete(&invalid).await.unwrap_err();
    assert!(matches!(err, VfsError::Unsupported(_)));
}

// ---------------------------------------------------------------------------
// rename
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rename_file() {
    let vfs = setup_fs().await;
    let file = vfs.new_item(Path::new("/file_a.txt")).await;
    vfs.rename(&file, "renamed.txt").await.unwrap();
    let renamed = vfs.new_item(Path::new("/renamed.txt")).await;
    assert!(renamed.is_file());
    assert_eq!(vfs.read_file(&renamed).await.unwrap(), "hello world");
    let gone = vfs.new_item(Path::new("/file_a.txt")).await;
    assert!(matches!(gone.kind, FSItemKind::Invalid { .. }));
}

#[tokio::test]
async fn test_rename_dir() {
    let vfs = setup_fs().await;
    let dir = vfs.new_item(Path::new("/dir_a")).await;
    vfs.rename(&dir, "dir_renamed").await.unwrap();
    let renamed = vfs.new_item(Path::new("/dir_renamed")).await;
    assert!(renamed.is_dir());
    let nested = vfs.new_item(Path::new("/dir_renamed/nested.txt")).await;
    assert!(nested.is_file());
    let gone = vfs.new_item(Path::new("/dir_a")).await;
    assert!(matches!(gone.kind, FSItemKind::Invalid { .. }));
}

#[tokio::test]
async fn test_rename_non_existent() {
    let vfs = setup_fs().await;
    let invalid = vfs.new_item(Path::new("/nonexistent")).await;
    let err = vfs.rename(&invalid, "newname").await.unwrap_err();
    assert!(matches!(
        err,
        VfsError::Io(ref e) if e.kind() == io::ErrorKind::NotFound
    ));
}
