;; A bot that throws and catches in every turn, in the standard (exnref) encoding of wasm exception
;; handling: `try_table`, `throw`, `throw_ref`. This is what clang emits for C++ exceptions with
;; `-fwasm-exceptions -mllvm -wasm-use-legacy-eh=false`.
;;
;; Each turn `$relay` calls `$descend`, which recurses three calls deep and throws `$miss` with payload 3.
;; `$relay` catches it with `catch_all_ref` and throws the same exception on with `throw_ref`; the turn
;; loop catches it with `catch $miss`, adds the payload to its count, writes the count (4 bytes, little
;; endian) to stdout, moves and ends its turn.
;;
;; Played by crates/runner/tests/exceptions.rs and accepted by the upload check in
;; crates/coordinator/src/wasmcheck.rs: the two must agree on it.
(module
  (import "ibc" "end_turn" (func $end_turn))
  (import "ibc" "act" (func $act (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "\00\00\00\00\04\00\00\00") ;; iovec { ptr = 0, len = 4 }
  (tag $miss (param i32))

  (func $descend (param $n i32)
    (if (i32.eqz (local.get $n)) (then (throw $miss (i32.const 3))))
    (call $descend (i32.sub (local.get $n) (i32.const 1))))

  (func $relay
    (throw_ref
      (block $caught (result exnref)
        (try_table (catch_all_ref $caught) (call $descend (i32.const 3)))
        (return))))

  (func (export "_start") (local $count i32) (local $angle i32)
    (loop $turn
      (local.set $count
        (i32.add (local.get $count)
          (block $handled (result i32)
            (try_table (catch $miss $handled) (call $relay))
            (unreachable))))
      (i32.store (i32.const 0) (local.get $count))
      (drop (call $fd_write (i32.const 1) (i32.const 16) (i32.const 1) (i32.const 32)))
      ;; A_MOVE, turning a little every tick
      (local.set $angle (i32.and (i32.add (local.get $angle) (i32.const 4099)) (i32.const 65535)))
      (drop (call $act (i32.const 1) (local.get $angle) (i32.const 300) (i32.const 0)))
      (call $end_turn)
      (br $turn))))
