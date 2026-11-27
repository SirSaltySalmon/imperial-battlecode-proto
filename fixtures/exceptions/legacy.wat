;; A throw and catch in the legacy encoding of wasm exception handling (`try` / `catch`), which clang
;; emits for `-fwasm-exceptions` unless given `-mllvm -wasm-use-legacy-eh=false`. Wasmtime cannot run it,
;; so the runner and the upload check in crates/coordinator/src/wasmcheck.rs both refuse it.
(module
  (import "ibc" "end_turn" (func $end_turn))
  (memory (export "memory") 1)
  (tag $miss (param i32))
  (func (export "_start")
    loop $turn
      try
        i32.const 3
        throw $miss
      catch $miss
        drop
      end
      call $end_turn
      br $turn
    end))
