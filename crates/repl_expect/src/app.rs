use std::cell::RefCell;

use roc_parse::ast::Expr;
use roc_repl_eval::{ReplApp, ReplAppMemory};
use roc_std::RocStr;
use roc_target::TargetInfo;

pub(crate) struct ExpectMemory {
    pub(crate) start: *const u8,
    pub(crate) bytes_read: RefCell<usize>,
}

macro_rules! deref_number {
    ($name: ident, $t: ty) => {
        fn $name(&self, addr: usize) -> $t {
            let ptr = unsafe { self.start.add(addr) } as *const _;
            *self.bytes_read.borrow_mut() += std::mem::size_of::<$t>();
            unsafe { std::ptr::read_unaligned(ptr) }
        }
    };
}

impl ReplAppMemory for ExpectMemory {
    deref_number!(deref_bool, bool);

    deref_number!(deref_u8, u8);
    deref_number!(deref_u16, u16);
    deref_number!(deref_u32, u32);
    deref_number!(deref_u64, u64);
    deref_number!(deref_u128, u128);
    deref_number!(deref_usize, usize);

    deref_number!(deref_i8, i8);
    deref_number!(deref_i16, i16);
    deref_number!(deref_i32, i32);
    deref_number!(deref_i64, i64);
    deref_number!(deref_i128, i128);
    deref_number!(deref_isize, isize);

    deref_number!(deref_f32, f32);
    deref_number!(deref_f64, f64);

    fn deref_str(&self, addr: usize) -> &str {
        const WIDTH: usize = 3 * std::mem::size_of::<usize>();

        let last_byte_addr = addr + WIDTH - 1;
        let last_byte = self.deref_i8(last_byte_addr);

        let is_small = last_byte < 0;

        if is_small {
            let ptr = unsafe { self.start.add(addr) };
            let roc_str: &RocStr = unsafe { &*ptr.cast() };

            *self.bytes_read.borrow_mut() += WIDTH - 1;

            roc_str.as_str()
        } else {
            let offset = self.deref_usize(addr);
            let length = self.deref_usize(addr + std::mem::size_of::<usize>());
            let _capacity = self.deref_usize(addr + 2 * std::mem::size_of::<usize>());

            // subtract the last byte, which we've now read twice
            *self.bytes_read.borrow_mut() -= 1;

            unsafe {
                let ptr = self.start.add(offset);
                let slice = std::slice::from_raw_parts(ptr, length);

                std::str::from_utf8_unchecked(slice)
            }
        }
    }
}

pub(crate) struct ExpectReplApp<'a> {
    pub(crate) memory: &'a ExpectMemory,
    pub(crate) offset: usize,
}

impl<'a> ReplApp<'a> for ExpectReplApp<'a> {
    type Memory = ExpectMemory;

    /// Run user code that returns a type with a `Builtin` layout
    /// Size of the return value is statically determined from its Rust type
    /// The `transform` callback takes the app's memory and the returned value
    /// _main_fn_name is always the same and we don't use it here
    fn call_function<Return, F>(&mut self, _main_fn_name: &str, transform: F) -> Expr<'a>
    where
        F: Fn(&'a Self::Memory, Return) -> Expr<'a>,
        Self::Memory: 'a,
    {
        let result: Return = unsafe {
            let ptr = self.memory.start.add(self.offset);
            let ptr: *const Return = std::mem::transmute(ptr);
            ptr.read()
        };

        self.offset += std::mem::size_of::<Return>();

        *self.memory.bytes_read.borrow_mut() = 0;

        let transformed = transform(self.memory, result);

        self.offset += *self.memory.bytes_read.borrow();

        transformed
    }

    fn call_function_returns_roc_list<F>(&mut self, main_fn_name: &str, transform: F) -> Expr<'a>
    where
        F: Fn(&'a Self::Memory, (usize, usize, usize)) -> Expr<'a>,
        Self::Memory: 'a,
    {
        self.call_function(main_fn_name, transform)
    }

    fn call_function_returns_roc_str<T, F>(
        &mut self,
        _target_info: TargetInfo,
        main_fn_name: &str,
        transform: F,
    ) -> T
    where
        F: Fn(&'a Self::Memory, usize) -> T,
        Self::Memory: 'a,
    {
        let string_length = RefCell::new(0);

        let result = self.call_function_dynamic_size(main_fn_name, 24, |memory, addr| {
            let last_byte_addr = addr + (3 * std::mem::size_of::<usize>()) - 1;
            let last_byte = memory.deref_i8(last_byte_addr);

            let is_small = last_byte < 0;

            if !is_small {
                let length = memory.deref_usize(addr + std::mem::size_of::<usize>());
                *string_length.borrow_mut() = length;
            }

            transform(memory, addr)
        });

        self.offset += *string_length.borrow();

        result
    }

    /// Run user code that returns a struct or union, whose size is provided as an argument
    /// The `transform` callback takes the app's memory and the address of the returned value
    /// _main_fn_name and _ret_bytes are only used for the CLI REPL. For Wasm they are compiled-in
    /// to the test_wrapper function of the app itself
    fn call_function_dynamic_size<T, F>(
        &mut self,
        _main_fn_name: &str,
        _ret_bytes: usize,
        transform: F,
    ) -> T
    where
        F: Fn(&'a Self::Memory, usize) -> T,
        Self::Memory: 'a,
    {
        *self.memory.bytes_read.borrow_mut() = 0;

        let result = transform(self.memory, self.offset);

        self.offset += *self.memory.bytes_read.borrow();

        result
    }
}
