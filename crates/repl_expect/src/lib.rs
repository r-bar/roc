use std::cell::RefCell;

use roc_module::symbol::Interns;
use roc_mono::{
    ir::ProcLayout,
    layout::{CapturesNiche, LayoutCache},
};
use roc_parse::ast::Expr;
use roc_repl_eval::eval::{jit_to_ast, ToAstProblem};
use roc_target::TargetInfo;
use roc_types::subs::{Subs, Variable};

mod app;
pub mod run;

use app::{ExpectMemory, ExpectReplApp};

#[allow(clippy::too_many_arguments)]
pub fn get_values<'a>(
    target_info: TargetInfo,
    arena: &'a bumpalo::Bump,
    subs: &Subs,
    interns: &'a Interns,
    start: *const u8,
    start_offset: usize,
    variables: &[Variable],
) -> Result<(usize, Vec<Expr<'a>>), ToAstProblem> {
    let mut result = Vec::with_capacity(variables.len());

    let memory = ExpectMemory {
        start,
        bytes_read: RefCell::new(0),
    };

    let app = ExpectReplApp {
        memory: arena.alloc(memory),
        offset: start_offset,
    };

    let app = arena.alloc(app);

    for variable in variables {
        let expr = {
            let variable = *variable;

            let content = subs.get_content_without_compacting(variable);

            let mut layout_cache = LayoutCache::new(target_info);
            let layout = layout_cache.from_var(arena, variable, subs).unwrap();

            let proc_layout = ProcLayout {
                arguments: &[],
                result: layout,
                captures_niche: CapturesNiche::no_niche(),
            };

            let element = jit_to_ast(
                arena,
                app,
                "expect_repl_main_fn",
                proc_layout,
                content,
                subs,
                interns,
                target_info,
            )?;

            element
        };

        result.push(expr);
    }

    Ok((app.offset, result))
}

#[cfg(test)]
mod test {
    use std::ffi::CString;

    use indoc::indoc;
    use pretty_assertions::assert_eq;
    use roc_gen_llvm::{llvm::build::LlvmBackendMode, run_roc::RocCallResult, run_roc_dylib};
    use roc_load::Threading;
    use roc_reporting::report::RenderTarget;
    use target_lexicon::Triple;

    use crate::run::expect_mono_module_to_dylib;

    use super::*;

    fn src_hash(src: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hash_state = DefaultHasher::new();
        src.hash(&mut hash_state);
        hash_state.finish()
    }

    fn run_expect_test(source: &str, expected: &str) {
        let arena = bumpalo::Bump::new();
        let arena = &arena;

        let triple = Triple::host();
        let target = &triple;

        let opt_level = roc_mono::ir::OptLevel::Normal;
        let target_info = TargetInfo::from(target);

        // Step 1: compile the app and generate the .o file
        let src_dir = tempfile::tempdir().unwrap();
        let filename = src_dir.path().join("Test.roc");

        std::fs::write(&filename, source).unwrap();

        let loaded = roc_load::load_and_monomorphize_from_str(
            arena,
            filename,
            source,
            src_dir.path().to_path_buf(),
            Default::default(),
            target_info,
            RenderTarget::ColorTerminal,
            Threading::Single,
        )
        .unwrap();

        let mut loaded = loaded;
        let mut expectations = std::mem::take(&mut loaded.expectations);
        let loaded = loaded;

        let interns = loaded.interns.clone();

        let (lib, expects) = expect_mono_module_to_dylib(
            arena,
            target.clone(),
            loaded,
            opt_level,
            LlvmBackendMode::CliTest,
        )
        .unwrap();

        let arena = &bumpalo::Bump::new();
        let interns = arena.alloc(interns);

        // IMPORTANT: shared memory object names must begin with / and contain no other slashes!
        let name = format!("/roc_expect_{}", src_hash(source));
        let cstring = CString::new(&*name).unwrap();

        const SHM_SIZE: i64 = 4096;

        let shared_ptr = unsafe {
            let shared_fd =
                libc::shm_open(cstring.as_ptr().cast(), libc::O_RDWR | libc::O_CREAT, 0o666);

            libc::ftruncate(shared_fd, SHM_SIZE);

            let ptr = libc::mmap(
                std::ptr::null_mut(),
                SHM_SIZE as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                shared_fd,
                0,
            );

            if ptr as isize == -1 {
                panic!("could not set up the expect shared memory region")
            }

            ptr.cast()
        };

        // communicate the mmapped name to zig/roc
        let set_mmapped_file = run_roc_dylib!(lib, "set_mmapped_file", (*const i8, usize), ());
        let mut result = RocCallResult::default();
        unsafe { set_mmapped_file((cstring.as_ptr(), name.len() + 1), &mut result) };
        std::mem::forget(cstring);

        let mut writer = Vec::with_capacity(1024);
        let (_failed, _passed) = crate::run::run_expects(
            &mut writer,
            RenderTarget::ColorTerminal,
            arena,
            interns,
            &lib,
            &mut expectations,
            shared_ptr,
            expects,
        )
        .unwrap();

        // Remove ANSI escape codes from the answer - for example:
        //
        //     Before: "42 \u{1b}[35m:\u{1b}[0m Num *"
        //     After:  "42 : Num *"
        let bytes = strip_ansi_escapes::strip(writer).unwrap();
        let actual = String::from_utf8(bytes).unwrap();

        if !actual.is_empty() {
            // trim off the first line; it contains a path in a tempdir that
            // changes between test runs
            let p = actual.bytes().position(|c| c == b'\n').unwrap();
            let (_, x) = actual.split_at(p);
            let x = x.trim_start();

            if x != expected {
                println!("{}", x);
            }

            assert_eq!(x, expected);
        } else {
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn equals_pass() {
        run_expect_test(
            r#"
            app "test" provides [main] to "./platform"

            main = 0

            expect 1 == 1
            "#,
            "",
        );
    }

    #[test]
    fn equals_fail() {
        run_expect_test(
            indoc!(
                r#"
                app "test" provides [main] to "./platform"

                main = 0

                expect 1 == 2
                "#
            ),
            indoc!(
                r#"
                This expectation failed:

                5│  expect 1 == 2
                    ^^^^^^^^^^^^^
                "#
            ),
        );
    }

    #[test]
    fn lookup_integer() {
        run_expect_test(
            indoc!(
                r#"
                app "test" provides [main] to "./platform"

                main = 0

                expect
                    a = 1
                    b = 2

                    a == b
                "#
            ),
            indoc!(
                r#"
                This expectation failed:

                5│>  expect
                6│>      a = 1
                7│>      b = 2
                8│>
                9│>      a == b

                When it failed, these variables had these values:

                a : Num a
                a = 1

                b : Num a
                b = 2
                "#
            ),
        );
    }

    #[test]
    fn lookup_list_of_strings() {
        run_expect_test(
            indoc!(
                r#"
                app "test" provides [main] to "./platform"

                main = 0

                expect
                    a = ["foo"]
                    b = ["a string so long that it cannot be short"]

                    a == b
                "#
            ),
            indoc!(
                r#"
                This expectation failed:

                5│>  expect
                6│>      a = ["foo"]
                7│>      b = ["a string so long that it cannot be short"]
                8│>
                9│>      a == b

                When it failed, these variables had these values:

                a : List Str
                a = ["foo"]

                b : List Str
                b = ["a string so long that it cannot be short"]
                "#
            ),
        );
    }

    #[test]
    #[ignore]
    fn lookup_list_of_list_of_strings() {
        run_expect_test(
            indoc!(
                r#"
                app "test" provides [main] to "./platform"

                main = 0

                expect
                    a = [["foo"], []]
                    b = [["a string so long that it cannot be short", "bar"]]

                    a == b
                "#
            ),
            indoc!(
                r#"
                This expectation failed:

                5│>  expect
                6│>      a = ["foo"]
                7│>      b = ["a string so long that it cannot be short"]
                8│>
                9│>      a == b

                When it failed, these variables had these values:

                `a` : `List` `Str`
                `a` = ["foo"]

                `b` : `List` `Str`
                `b` = ["a string so long that it cannot be short"]
                "#
            ),
        );
    }

    #[test]
    #[ignore]
    fn lookup_result() {
        run_expect_test(
            indoc!(
                r#"
                app "test" provides [main] to "./platform"

                main = 0

                expect
                    items = [0, 1]
                    expected : Result I64 [OutOfBounds]*
                    expected = Ok 42


                    List.get items 0 == expected
                "#
            ),
            indoc!(
                r#"
                This expectation failed:

                 5│>  expect
                 6│>      items = [0, 1]
                 7│>      expected = Ok 42
                 8│>
                 9│>
                10│>      List.get items 0 == expected

                When it failed, these variables had these values:

                items : List (Num a)
                items = [0, 1]

                expected : [Ok (Num a)]b
                expected = Ok 140526413778648
                "#
            ),
        );
    }

    #[test]
    fn lookup_copy_record() {
        run_expect_test(
            indoc!(
                r#"
                app "test" provides [main] to "./platform"

                main = 0

                expect
                    vec1 = { x: 1.0, y: 2.0 }
                    vec2 = { x: 4.0, y: 8.0 }

                    vec1 == vec2
                "#
            ),
            indoc!(
                r#"
                This expectation failed:

                5│>  expect
                6│>      vec1 = { x: 1.0, y: 2.0 }
                7│>      vec2 = { x: 4.0, y: 8.0 }
                8│>
                9│>      vec1 == vec2

                When it failed, these variables had these values:

                vec1 : { x : Frac a, y : Frac b }
                vec1 = { x: 1, y: 2 }

                vec2 : { x : Frac a, y : Frac b }
                vec2 = { x: 4, y: 8 }
                "#
            ),
        );
    }
}
