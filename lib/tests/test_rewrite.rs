// Copyright 2021 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use itertools::Itertools as _;
use jj_lib::commit::Commit;
use jj_lib::matchers::{EverythingMatcher, FilesMatcher};
use jj_lib::merged_tree::MergedTree;
use jj_lib::op_store::{RefTarget, RemoteRef, RemoteRefState, WorkspaceId};
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::rewrite::{
    restore_tree, DescendantRebaser, EmptyBehaviour, RebaseOptions, RebasedDescendant,
};
use maplit::{hashmap, hashset};
use test_case::test_case;
use testutils::{
    assert_rebased, create_random_commit, create_tree, write_random_commit, CommitGraphBuilder,
    TestRepo,
};

#[test]
fn test_restore_tree() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let path1 = RepoPath::from_internal_string("file1");
    let path2 = RepoPath::from_internal_string("dir1/file2");
    let path3 = RepoPath::from_internal_string("dir1/file3");
    let path4 = RepoPath::from_internal_string("dir2/file4");
    let left = create_tree(repo, &[(path2, "left"), (path3, "left"), (path4, "left")]);
    let right = create_tree(
        repo,
        &[(path1, "right"), (path2, "right"), (path3, "right")],
    );

    // Restore everything using EverythingMatcher
    let restored = restore_tree(&left, &right, &EverythingMatcher).unwrap();
    assert_eq!(restored, left.id());

    // Restore everything using FilesMatcher
    let restored = restore_tree(
        &left,
        &right,
        &FilesMatcher::new([&path1, &path2, &path3, &path4]),
    )
    .unwrap();
    assert_eq!(restored, left.id());

    // Restore some files
    let restored = restore_tree(&left, &right, &FilesMatcher::new([path1, path2])).unwrap();
    let expected = create_tree(repo, &[(path2, "left"), (path3, "right")]);
    assert_eq!(restored, expected.id());
}

#[test]
fn test_rebase_descendants_sideways() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit F. Commits C-E should be rebased.
    //
    // F
    // | D
    // | C E
    // | |/
    // | B
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_a]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_f.id().clone()}
        },
        hashset! {},
    );
    let new_commit_c = assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_f]);
    let new_commit_d = assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&new_commit_c]);
    let new_commit_e = assert_rebased(rebaser.rebase_next().unwrap(), &commit_e, &[&commit_f]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 3);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_d.id().clone(),
            new_commit_e.id().clone()
        }
    );
}

#[test]
fn test_rebase_descendants_forward() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit F. Commits C and E should be rebased onto F.
    // Commit D does not get rebased because it's an ancestor of the
    // destination. Commit G does not get replaced because it's already in
    // place.
    // TODO: The above is not what actually happens! The test below shows what
    // actually happens: D and F also get rebased onto F, so we end up with
    // duplicates. Consider if it's worth supporting the case above better or if
    // that decision belongs with the caller (as we currently force it to do by
    // not supporting it in DescendantRebaser).
    //
    // G
    // F E
    // |/
    // D C
    // |/
    // B
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_g = graph_builder.commit_with_parents(&[&commit_f]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_f.id().clone()}
        },
        hashset! {},
    );
    let new_commit_d = assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&commit_f]);
    let new_commit_f = assert_rebased(rebaser.rebase_next().unwrap(), &commit_f, &[&new_commit_d]);
    let new_commit_c = assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&new_commit_f]);
    let new_commit_e = assert_rebased(rebaser.rebase_next().unwrap(), &commit_e, &[&new_commit_d]);
    let new_commit_g = assert_rebased(rebaser.rebase_next().unwrap(), &commit_g, &[&new_commit_f]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 5);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_c.id().clone(),
            new_commit_e.id().clone(),
            new_commit_g.id().clone(),
        }
    );
}

#[test]
fn test_rebase_descendants_reorder() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit E was replaced by commit D, and commit C was replaced by commit F
    // (attempting to to reorder C and E), and commit G was replaced by commit
    // H.
    //
    // I
    // G H
    // E F
    // C D
    // |/
    // B
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_g = graph_builder.commit_with_parents(&[&commit_e]);
    let commit_h = graph_builder.commit_with_parents(&[&commit_f]);
    let commit_i = graph_builder.commit_with_parents(&[&commit_g]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_e.id().clone() => hashset!{commit_d.id().clone()},
            commit_c.id().clone() => hashset!{commit_f.id().clone()},
            commit_g.id().clone() => hashset!{commit_h.id().clone()},
        },
        hashset! {},
    );
    let new_commit_i = assert_rebased(rebaser.rebase_next().unwrap(), &commit_i, &[&commit_h]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 1);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_i.id().clone(),
        }
    );
}

#[test]
fn test_rebase_descendants_backward() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit C was replaced by commit B. Commit D should be rebased.
    //
    // D
    // C
    // B
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_c.id().clone() => hashset!{commit_b.id().clone()}
        },
        hashset! {},
    );
    let new_commit_d = assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&commit_b]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 1);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {new_commit_d.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_chain_becomes_branchy() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit E and commit C was replaced by commit F.
    // Commit F should get rebased onto E, and commit D should get rebased onto
    // the rebased F.
    //
    // D
    // C F
    // |/
    // B E
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_b]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_e.id().clone()},
            commit_c.id().clone() => hashset!{commit_f.id().clone()},
        },
        hashset! {},
    );
    let new_commit_f = assert_rebased(rebaser.rebase_next().unwrap(), &commit_f, &[&commit_e]);
    let new_commit_d = assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&new_commit_f]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 2);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_d.id().clone(),
        }
    );
}

#[test]
fn test_rebase_descendants_internal_merge() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit F. Commits C-E should be rebased.
    //
    // F
    // | E
    // | |\
    // | C D
    // | |/
    // | B
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_c, &commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_a]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_f.id().clone()}
        },
        hashset! {},
    );
    let new_commit_c = assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_f]);
    let new_commit_d = assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&commit_f]);
    let new_commit_e = assert_rebased(
        rebaser.rebase_next().unwrap(),
        &commit_e,
        &[&new_commit_c, &new_commit_d],
    );
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 3);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! { new_commit_e.id().clone() }
    );
}

#[test]
fn test_rebase_descendants_external_merge() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit C was replaced by commit F. Commits E should be rebased. The rebased
    // commit E should have F as first parent and commit D as second parent.
    //
    // F
    // | E
    // | |\
    // | C D
    // | |/
    // | B
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_c, &commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_a]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_c.id().clone() => hashset!{commit_f.id().clone()}
        },
        hashset! {},
    );
    let new_commit_e = assert_rebased(
        rebaser.rebase_next().unwrap(),
        &commit_e,
        &[&commit_f, &commit_d],
    );
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 1);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {new_commit_e.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_abandon() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B and commit E were abandoned. Commit C and commit D should get
    // rebased onto commit A. Commit F should get rebased onto the new commit D.
    //
    // F
    // E
    // D C
    // |/
    // B
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_e]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {},
        hashset! {commit_b.id().clone(), commit_e.id().clone()},
    );
    let new_commit_c = assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_a]);
    let new_commit_d = assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&commit_a]);
    let new_commit_f = assert_rebased(rebaser.rebase_next().unwrap(), &commit_f, &[&new_commit_d]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 3);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_c.id().clone(),
            new_commit_f.id().clone()
        }
    );
}

#[test]
fn test_rebase_descendants_abandon_no_descendants() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B and C were abandoned. Commit A should become a head.
    //
    // C
    // B
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {},
        hashset! {commit_b.id().clone(), commit_c.id().clone()},
    );
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 0);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            commit_a.id().clone(),
        }
    );
}

#[test]
fn test_rebase_descendants_abandon_and_replace() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit E. Commit C was abandoned. Commit D should
    // get rebased onto commit E.
    //
    //   D
    //   C
    // E B
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_a]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {commit_b.id().clone() => hashset!{commit_e.id().clone()}},
        hashset! {commit_c.id().clone()},
    );
    let new_commit_d = assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&commit_e]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 1);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! { new_commit_d.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_abandon_degenerate_merge() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was abandoned. Commit D should get rebased to have only C as parent
    // (not A and C).
    //
    // D
    // |\
    // B C
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {},
        hashset! {commit_b.id().clone()},
    );
    let new_commit_d = assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&commit_c]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 1);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {new_commit_d.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_abandon_widen_merge() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit E was abandoned. Commit F should get rebased to have B, C, and D as
    // parents (in that order).
    //
    // F
    // |\
    // E \
    // |\ \
    // B C D
    //  \|/
    //   A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_e, &commit_d]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {},
        hashset! {commit_e.id().clone()},
    );
    let new_commit_f = assert_rebased(
        rebaser.rebase_next().unwrap(),
        &commit_f,
        &[&commit_b, &commit_c, &commit_d],
    );
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 1);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! { new_commit_f.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_multiple_sideways() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B and commit D were both replaced by commit F. Commit C and commit E
    // should get rebased onto it.
    //
    // C E
    // B D F
    // | |/
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_a]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_f.id().clone()},
            commit_d.id().clone() => hashset!{commit_f.id().clone()},
        },
        hashset! {},
    );
    let new_commit_c = assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_f]);
    let new_commit_e = assert_rebased(rebaser.rebase_next().unwrap(), &commit_e, &[&commit_f]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 2);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_c.id().clone(),
            new_commit_e.id().clone()
        }
    );
}

#[test]
#[should_panic(expected = "cycle detected")]
fn test_rebase_descendants_multiple_swap() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit D. Commit D was replaced by commit B.
    // This results in an infinite loop and a panic
    //
    // C E
    // B D
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let _commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_a]);
    let _commit_e = graph_builder.commit_with_parents(&[&commit_d]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_d.id().clone()},
            commit_d.id().clone() => hashset!{commit_b.id().clone()},
        },
        hashset! {},
    );
    let _ = rebaser.rebase_next(); // Panics because of the cycle
}

#[test]
fn test_rebase_descendants_multiple_no_descendants() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit C. Commit C was replaced by commit B.
    //
    // B C
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_c.id().clone()},
            commit_c.id().clone() => hashset!{commit_b.id().clone()},
        },
        hashset! {},
    );
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert!(rebaser.rebased().is_empty());

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            commit_b.id().clone(),
            commit_c.id().clone()
        }
    );
}

#[test]
fn test_rebase_descendants_divergent_rewrite() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit B2. Commit D was replaced by commits D2 and
    // D3. Commit F was replaced by commit F2. Commit C should be rebased onto
    // B2. Commit E should not be rebased. Commit G should be rebased onto
    // commit F2.
    //
    // G
    // F
    // E
    // D
    // C
    // B
    // | F2
    // |/
    // | D3
    // |/
    // | D2
    // |/
    // | B2
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_e]);
    let commit_g = graph_builder.commit_with_parents(&[&commit_f]);
    let commit_b2 = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d2 = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d3 = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_f2 = graph_builder.commit_with_parents(&[&commit_a]);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_b2.id().clone()},
            commit_d.id().clone() => hashset!{commit_d2.id().clone(), commit_d3.id().clone()},
            commit_f.id().clone() => hashset!{commit_f2.id().clone()},
        },
        hashset! {},
    );
    let new_commit_c = assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_b2]);
    let new_commit_g = assert_rebased(rebaser.rebase_next().unwrap(), &commit_g, &[&commit_f2]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 2);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_c.id().clone(),
            commit_d2.id().clone(),
            commit_d3.id().clone(),
            commit_e.id().clone(),
            new_commit_g.id().clone(),
        }
    );
}

#[test]
fn test_rebase_descendants_repeated() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit B2. Commit C should get rebased. Rebasing
    // descendants again should have no effect (C should not get rebased again).
    // We then replace B2 by B3. C should now get rebased onto B3.
    //
    // C
    // B
    // | B3
    // |/
    // | B2
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);

    let commit_b2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .set_description("b2")
        .write()
        .unwrap();
    let mut rebaser = tx.mut_repo().create_descendant_rebaser(&settings);
    let commit_c2 = assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_b2]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 1);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            commit_c2.id().clone(),
        }
    );

    // We made no more changes, so nothing should be rebased.
    let mut rebaser = tx.mut_repo().create_descendant_rebaser(&settings);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 0);

    // Now mark B3 as rewritten from B2 and rebase descendants again.
    let commit_b3 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b2)
        .set_description("b3")
        .write()
        .unwrap();
    let mut rebaser = tx.mut_repo().create_descendant_rebaser(&settings);
    let commit_c3 = assert_rebased(rebaser.rebase_next().unwrap(), &commit_c2, &[&commit_b3]);
    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 1);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            // commit_b.id().clone(),
            commit_c3.id().clone(),
        }
    );
}

#[test]
fn test_rebase_descendants_contents() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Commit B was replaced by commit D. Commit C should have the changes from
    // commit C and commit D, but not the changes from commit B.
    //
    // D
    // | C
    // | B
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let path1 = RepoPath::from_internal_string("file1");
    let tree1 = create_tree(repo, &[(path1, "content")]);
    let commit_a = tx
        .mut_repo()
        .new_commit(
            &settings,
            vec![repo.store().root_commit_id().clone()],
            tree1.id(),
        )
        .write()
        .unwrap();
    let path2 = RepoPath::from_internal_string("file2");
    let tree2 = create_tree(repo, &[(path2, "content")]);
    let commit_b = tx
        .mut_repo()
        .new_commit(&settings, vec![commit_a.id().clone()], tree2.id())
        .write()
        .unwrap();
    let path3 = RepoPath::from_internal_string("file3");
    let tree3 = create_tree(repo, &[(path3, "content")]);
    let commit_c = tx
        .mut_repo()
        .new_commit(&settings, vec![commit_b.id().clone()], tree3.id())
        .write()
        .unwrap();
    let path4 = RepoPath::from_internal_string("file4");
    let tree4 = create_tree(repo, &[(path4, "content")]);
    let commit_d = tx
        .mut_repo()
        .new_commit(&settings, vec![commit_a.id().clone()], tree4.id())
        .write()
        .unwrap();

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_d.id().clone()}
        },
        hashset! {},
    );
    rebaser.rebase_all().unwrap();
    let rebased = rebaser.rebased();
    assert_eq!(rebased.len(), 1);
    let new_commit_c = repo
        .store()
        .get_commit(rebased.get(commit_c.id()).unwrap())
        .unwrap();

    let tree_b = commit_b.tree().unwrap();
    let tree_c = commit_c.tree().unwrap();
    let tree_d = commit_d.tree().unwrap();
    let new_tree_c = new_commit_c.tree().unwrap();
    assert_eq!(new_tree_c.path_value(path3), tree_c.path_value(path3));
    assert_eq!(new_tree_c.path_value(path4), tree_d.path_value(path4));
    assert_ne!(new_tree_c.path_value(path2), tree_b.path_value(path2));
}

#[test]
fn test_rebase_descendants_basic_branch_update() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Branch "main" points to commit B. B gets rewritten as B2. Branch main should
    // be updated to point to B2.
    //
    // B main         B2 main
    // |         =>   |
    // A              A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    tx.mut_repo()
        .set_local_branch_target("main", RefTarget::normal(commit_b.id().clone()));
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    let commit_b2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .write()
        .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert_eq!(
        tx.mut_repo().get_local_branch("main"),
        RefTarget::normal(commit_b2.id().clone())
    );

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {commit_b2.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_branch_move_two_steps() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Branch "main" points to branch C. C gets rewritten as C2 and B gets rewritten
    // as B2. C2 should be rebased onto B2, creating C3, and main should be
    // updated to point to C3.
    //
    // C2 C main      C3 main
    // | /            |
    // |/        =>   |
    // B B2           B2
    // |/             |
    // A              A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    tx.mut_repo()
        .set_local_branch_target("main", RefTarget::normal(commit_c.id().clone()));
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    let commit_b2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .write()
        .unwrap();
    let commit_c2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_c)
        .write()
        .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let heads = tx.mut_repo().view().heads();
    assert_eq!(heads.len(), 1);
    let c3_id = heads.iter().next().unwrap().clone();
    let commit_c3 = repo.store().get_commit(&c3_id).unwrap();
    assert_ne!(commit_c3.id(), commit_c2.id());
    assert_eq!(commit_c3.parent_ids(), vec![commit_b2.id().clone()]);
    assert_eq!(
        tx.mut_repo().get_local_branch("main"),
        RefTarget::normal(commit_c3.id().clone())
    );
}

#[test]
fn test_rebase_descendants_basic_branch_update_with_non_local_branch() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Branch "main" points to commit B. B gets rewritten as B2. Branch main should
    // be updated to point to B2. Remote branch main@origin and tag v1 should not
    // get updated.
    //
    //                                B2 main
    // B main main@origin v1          | B main@origin v1
    // |                         =>   |/
    // A                              A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_b_remote_ref = RemoteRef {
        target: RefTarget::normal(commit_b.id().clone()),
        state: RemoteRefState::Tracking,
    };
    tx.mut_repo()
        .set_local_branch_target("main", RefTarget::normal(commit_b.id().clone()));
    tx.mut_repo()
        .set_remote_branch("main", "origin", commit_b_remote_ref.clone());
    tx.mut_repo()
        .set_tag_target("v1", RefTarget::normal(commit_b.id().clone()));
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    let commit_b2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .write()
        .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert_eq!(
        tx.mut_repo().get_local_branch("main"),
        RefTarget::normal(commit_b2.id().clone())
    );
    // The remote branch and tag should not get updated
    assert_eq!(
        tx.mut_repo().get_remote_branch("main", "origin"),
        commit_b_remote_ref,
    );
    assert_eq!(
        tx.mut_repo().get_tag("v1"),
        RefTarget::normal(commit_b.id().clone())
    );

    // Commit B is no longer visible even though the remote branch points to it.
    // (The user can still see it using e.g. the `remote_branches()` revset.)
    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {commit_b2.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_update_branch_after_abandon() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Branch "main" points to commit B. B is then abandoned. Branch main should
    // be updated to point to A.
    //
    // B main
    // |          =>   A main
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    tx.mut_repo()
        .set_local_branch_target("main", RefTarget::normal(commit_b.id().clone()));
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo().record_abandoned_commit(commit_b.id().clone());
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert_eq!(
        tx.mut_repo().get_local_branch("main"),
        RefTarget::normal(commit_a.id().clone())
    );

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {commit_a.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_update_branches_after_divergent_rewrite() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Branch "main" points to commit B. B gets rewritten as B2, B3, B4. Branch main
    // should become a conflict pointing to all of them.
    //
    //                B4 main?
    //                | B3 main?
    // B main         |/B2 main?
    // |         =>   |/
    // A              A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    tx.mut_repo()
        .set_local_branch_target("main", RefTarget::normal(commit_b.id().clone()));
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    let commit_b2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .write()
        .unwrap();
    // Different description so they're not the same commit
    let commit_b3 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .set_description("different")
        .write()
        .unwrap();
    // Different description so they're not the same commit
    let commit_b4 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .set_description("more different")
        .write()
        .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();

    let target = tx.mut_repo().get_local_branch("main");
    assert!(target.has_conflict());
    assert_eq!(
        target.removed_ids().counts(),
        hashmap! { commit_b.id() => 2 },
    );
    assert_eq!(
        target.added_ids().counts(),
        hashmap! {
            commit_b2.id() => 1,
            commit_b3.id() => 1,
            commit_b4.id() => 1,
        },
    );

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            commit_b2.id().clone(),
            commit_b3.id().clone(),
            commit_b4.id().clone(),
        }
    );
}

#[test]
fn test_rebase_descendants_rewrite_updates_branch_conflict() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Branch "main" is a conflict removing commit A and adding commits B and C.
    // A gets rewritten as A2 and A3. B gets rewritten as B2 and B2. The branch
    // should become a conflict removing A and B, and adding B2, B3, C.
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.initial_commit();
    tx.mut_repo().set_local_branch_target(
        "main",
        RefTarget::from_legacy_form(
            [commit_a.id().clone()],
            [commit_b.id().clone(), commit_c.id().clone()],
        ),
    );
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    let commit_a2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_a)
        .write()
        .unwrap();
    // Different description so they're not the same commit
    let commit_a3 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_a)
        .set_description("different")
        .write()
        .unwrap();
    let commit_b2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .write()
        .unwrap();
    // Different description so they're not the same commit
    let commit_b3 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .set_description("different")
        .write()
        .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();

    let target = tx.mut_repo().get_local_branch("main");
    assert!(target.has_conflict());
    assert_eq!(
        target.removed_ids().counts(),
        hashmap! { commit_a.id() => 1, commit_b.id() => 1 },
    );
    assert_eq!(
        target.added_ids().counts(),
        hashmap! {
            commit_c.id() => 1,
            commit_b2.id() => 1,
            commit_b3.id() => 1,
        },
    );

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            commit_a2.id().clone(),
            commit_a3.id().clone(),
            commit_b2.id().clone(),
            commit_b3.id().clone(),
            commit_c.id().clone(),
        }
    );
}

#[test]
fn test_rebase_descendants_rewrite_resolves_branch_conflict() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Branch "main" is a conflict removing ancestor commit A and adding commit B
    // and C (maybe it moved forward to B locally and moved forward to C
    // remotely). Now B gets rewritten as B2, which is a descendant of C (maybe
    // B was automatically rebased on top of the updated remote). That
    // would result in a conflict removing A and adding B2 and C. However, since C
    // is a descendant of A, and B2 is a descendant of C, the conflict gets
    // resolved to B2.
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    tx.mut_repo().set_local_branch_target(
        "main",
        RefTarget::from_legacy_form(
            [commit_a.id().clone()],
            [commit_b.id().clone(), commit_c.id().clone()],
        ),
    );
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    let commit_b2 = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .set_parents(vec![commit_c.id().clone()])
        .write()
        .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert_eq!(
        tx.mut_repo().get_local_branch("main"),
        RefTarget::normal(commit_b2.id().clone())
    );

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! { commit_b2.id().clone()}
    );
}

#[test]
fn test_rebase_descendants_branch_delete_modify_abandon() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Branch "main" initially points to commit A. One operation rewrites it to
    // point to B (child of A). A concurrent operation deletes the branch. That
    // leaves the branch pointing to "-A+B". We now abandon B. That should
    // result in the branch pointing to "-A+A=0", so the branch should
    // be deleted.
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    tx.mut_repo().set_local_branch_target(
        "main",
        RefTarget::from_legacy_form([commit_a.id().clone()], [commit_b.id().clone()]),
    );
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo().record_abandoned_commit(commit_b.id().clone());
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert_eq!(tx.mut_repo().get_local_branch("main"), RefTarget::absent());
}

#[test]
fn test_rebase_descendants_update_checkout() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Checked-out commit B was replaced by commit C. C should become
    // checked out.
    //
    // C B
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let commit_b = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![commit_a.id().clone()])
        .write()
        .unwrap();
    let ws1_id = WorkspaceId::new("ws1".to_string());
    let ws2_id = WorkspaceId::new("ws2".to_string());
    let ws3_id = WorkspaceId::new("ws3".to_string());
    tx.mut_repo()
        .set_wc_commit(ws1_id.clone(), commit_b.id().clone())
        .unwrap();
    tx.mut_repo()
        .set_wc_commit(ws2_id.clone(), commit_b.id().clone())
        .unwrap();
    tx.mut_repo()
        .set_wc_commit(ws3_id.clone(), commit_a.id().clone())
        .unwrap();
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    let commit_c = tx
        .mut_repo()
        .rewrite_commit(&settings, &commit_b)
        .set_description("C")
        .write()
        .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit("test");

    // Workspaces 1 and 2 had B checked out, so they get updated to C. Workspace 3
    // had A checked out, so it doesn't get updated.
    assert_eq!(repo.view().get_wc_commit_id(&ws1_id), Some(commit_c.id()));
    assert_eq!(repo.view().get_wc_commit_id(&ws2_id), Some(commit_c.id()));
    assert_eq!(repo.view().get_wc_commit_id(&ws3_id), Some(commit_a.id()));
}

#[test]
fn test_rebase_descendants_update_checkout_abandoned() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Checked-out commit B was abandoned. A child of A
    // should become checked out.
    //
    // B
    // |
    // A
    let mut tx = repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let commit_b = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![commit_a.id().clone()])
        .write()
        .unwrap();
    let ws1_id = WorkspaceId::new("ws1".to_string());
    let ws2_id = WorkspaceId::new("ws2".to_string());
    let ws3_id = WorkspaceId::new("ws3".to_string());
    tx.mut_repo()
        .set_wc_commit(ws1_id.clone(), commit_b.id().clone())
        .unwrap();
    tx.mut_repo()
        .set_wc_commit(ws2_id.clone(), commit_b.id().clone())
        .unwrap();
    tx.mut_repo()
        .set_wc_commit(ws3_id.clone(), commit_a.id().clone())
        .unwrap();
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo().record_abandoned_commit(commit_b.id().clone());
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit("test");

    // Workspaces 1 and 2 had B checked out, so they get updated to the same new
    // commit on top of C. Workspace 3 had A checked out, so it doesn't get updated.
    assert_eq!(
        repo.view().get_wc_commit_id(&ws1_id),
        repo.view().get_wc_commit_id(&ws2_id)
    );
    let checkout = repo
        .store()
        .get_commit(repo.view().get_wc_commit_id(&ws1_id).unwrap())
        .unwrap();
    assert_eq!(checkout.parent_ids(), vec![commit_a.id().clone()]);
    assert_eq!(repo.view().get_wc_commit_id(&ws3_id), Some(commit_a.id()));
}

#[test]
fn test_rebase_descendants_update_checkout_abandoned_merge() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Checked-out merge commit D was abandoned. A parent commit should become
    // checked out.
    //
    // D
    // |\
    // B C
    // |/
    // A
    let mut tx = repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let commit_b = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![commit_a.id().clone()])
        .write()
        .unwrap();
    let commit_c = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![commit_a.id().clone()])
        .write()
        .unwrap();
    let commit_d = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![commit_b.id().clone(), commit_c.id().clone()])
        .write()
        .unwrap();
    let workspace_id = WorkspaceId::default();
    tx.mut_repo()
        .set_wc_commit(workspace_id.clone(), commit_d.id().clone())
        .unwrap();
    let repo = tx.commit("test");

    let mut tx = repo.start_transaction(&settings);
    tx.mut_repo().record_abandoned_commit(commit_d.id().clone());
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit("test");

    let new_checkout_id = repo.view().get_wc_commit_id(&workspace_id).unwrap();
    let checkout = repo.store().get_commit(new_checkout_id).unwrap();
    assert_eq!(checkout.parent_ids(), vec![commit_b.id().clone()]);
}

fn assert_rebase_skipped(
    rebased: Option<RebasedDescendant>,
    expected_old_commit: &Commit,
    expected_new_commit: &Commit,
) -> Commit {
    if let Some(RebasedDescendant {
        old_commit,
        new_commit,
    }) = rebased
    {
        assert_eq!(old_commit, *expected_old_commit,);
        assert_eq!(new_commit, *expected_new_commit);
        // Since it was abandoned, the change ID should be different.
        assert_ne!(old_commit.change_id(), new_commit.change_id());
        new_commit
    } else {
        panic!("expected rebased commit: {rebased:?}");
    }
}

#[test_case(EmptyBehaviour::Keep; "keep all commits")]
#[test_case(EmptyBehaviour::AbandonNewlyEmpty; "abandon newly empty commits")]
#[test_case(EmptyBehaviour::AbandonAllEmpty ; "abandon all empty commits")]
fn test_empty_commit_option(empty: EmptyBehaviour) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Rebase a previously empty commit, a newly empty commit, and a commit with
    // actual changes.
    //
    // BD (commit B joined with commit D)
    // |   H (empty, no parent tree changes)
    // |   |
    // |   G
    // |   |
    // |   F (clean merge)
    // |  /|\
    // | C D E (empty, but parent tree changes)
    // |  \|/
    // |   B
    // A__/
    let mut tx = repo.start_transaction(&settings);
    let mut_repo = tx.mut_repo();
    let create_fixed_tree = |paths: &[&str]| {
        let content_map = paths
            .iter()
            .map(|&p| (RepoPath::from_internal_string(p), p))
            .collect_vec();
        create_tree(repo, &content_map)
    };

    // The commit_with_parents function generates non-empty merge commits, so it
    // isn't suitable for this test case.
    let tree_b = create_fixed_tree(&["B"]);
    let tree_c = create_fixed_tree(&["B", "C"]);
    let tree_d = create_fixed_tree(&["B", "D"]);
    let tree_f = create_fixed_tree(&["B", "C", "D"]);
    let tree_g = create_fixed_tree(&["B", "C", "D", "G"]);

    let commit_a = create_random_commit(mut_repo, &settings).write().unwrap();

    let mut create_commit = |parents: &[&Commit], tree: &MergedTree| {
        create_random_commit(mut_repo, &settings)
            .set_parents(
                parents
                    .iter()
                    .map(|commit| commit.id().clone())
                    .collect_vec(),
            )
            .set_tree_id(tree.id())
            .write()
            .unwrap()
    };
    let commit_b = create_commit(&[&commit_a], &tree_b);
    let commit_c = create_commit(&[&commit_b], &tree_c);
    let commit_d = create_commit(&[&commit_b], &tree_d);
    let commit_e = create_commit(&[&commit_b], &tree_b);
    let commit_f = create_commit(&[&commit_c, &commit_d, &commit_e], &tree_f);
    let commit_g = create_commit(&[&commit_f], &tree_g);
    let commit_h = create_commit(&[&commit_g], &tree_g);
    let commit_bd = create_commit(&[&commit_a], &tree_d);

    let mut rebaser = DescendantRebaser::new(
        &settings,
        tx.mut_repo(),
        hashmap! {
            commit_b.id().clone() => hashset!{commit_bd.id().clone()}
        },
        hashset! {},
    );
    *rebaser.mut_options() = RebaseOptions {
        empty: empty.clone(),
    };

    let new_head = match empty {
        EmptyBehaviour::Keep => {
            // The commit C isn't empty.
            let new_commit_c =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_bd]);
            let new_commit_d =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_d, &[&commit_bd]);
            let new_commit_e =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_e, &[&commit_bd]);
            let new_commit_f = assert_rebased(
                rebaser.rebase_next().unwrap(),
                &commit_f,
                &[&new_commit_c, &new_commit_d, &new_commit_e],
            );
            let new_commit_g =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_g, &[&new_commit_f]);
            assert_rebased(rebaser.rebase_next().unwrap(), &commit_h, &[&new_commit_g])
        }
        EmptyBehaviour::AbandonAllEmpty => {
            // The commit C isn't empty.
            let new_commit_c =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_bd]);
            // D and E are empty, and F is a clean merge with only one child. Thus, F is
            // also considered empty.
            assert_rebase_skipped(rebaser.rebase_next().unwrap(), &commit_d, &commit_bd);
            assert_rebase_skipped(rebaser.rebase_next().unwrap(), &commit_e, &commit_bd);
            assert_rebase_skipped(rebaser.rebase_next().unwrap(), &commit_f, &new_commit_c);
            let new_commit_g =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_g, &[&new_commit_c]);
            assert_rebase_skipped(rebaser.rebase_next().unwrap(), &commit_h, &new_commit_g)
        }
        EmptyBehaviour::AbandonNewlyEmpty => {
            // The commit C isn't empty.
            let new_commit_c =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_c, &[&commit_bd]);

            // The changes in D are included in BD, so D is newly empty.
            assert_rebase_skipped(rebaser.rebase_next().unwrap(), &commit_d, &commit_bd);
            // E was already empty, so F is a merge commit with C and E as parents.
            // Although it's empty, we still keep it because we don't want to drop merge
            // commits.
            let new_commit_e =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_e, &[&commit_bd]);
            let new_commit_f = assert_rebased(
                rebaser.rebase_next().unwrap(),
                &commit_f,
                &[&new_commit_c, &new_commit_e],
            );
            let new_commit_g =
                assert_rebased(rebaser.rebase_next().unwrap(), &commit_g, &[&new_commit_f]);
            assert_rebased(rebaser.rebase_next().unwrap(), &commit_h, &[&new_commit_g])
        }
    };

    assert!(rebaser.rebase_next().unwrap().is_none());
    assert_eq!(rebaser.rebased().len(), 6);

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_head.id().clone(),
        }
    );
}
