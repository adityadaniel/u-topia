;; Word count: a minimal Trellis plugin, written by hand in WebAssembly text.
;; It needs no permissions: it only reads its input and returns a message.
;;
;; Input (from the host): {"block":…,"command":"count","page":…,"text":"…"}
;; Output:                {"message":"N words"}
(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 16384))
  (data (i32.const 0) "\"text\":\"")        ;; 8 bytes: the key to find
  (data (i32.const 16) "{\"message\":\"")   ;; 12 bytes
  (data (i32.const 32) " words\"}")          ;; 8 bytes

  (func (export "trellis_alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (local.get $p))

  ;; Offset of the 8-byte key at address 0 within [p, p+n), or -1.
  (func $find (param $p i32) (param $n i32) (result i32)
    (local $i i32) (local $j i32)
    (block $done
      (loop $outer
        (br_if $done (i32.gt_s (i32.add (local.get $i) (i32.const 8)) (local.get $n)))
        (local.set $j (i32.const 0))
        (block $miss
          (loop $inner
            (br_if $miss
              (i32.ne
                (i32.load8_u (i32.add (local.get $p) (i32.add (local.get $i) (local.get $j))))
                (i32.load8_u (local.get $j))))
            (local.set $j (i32.add (local.get $j) (i32.const 1)))
            (if (i32.eq (local.get $j) (i32.const 8)) (then (return (local.get $i))))
            (br $inner)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $outer)))
    (i32.const -1))

  (func (export "trellis_run") (param $p i32) (param $n i32) (result i64)
    (local $i i32) (local $c i32) (local $words i32) (local $inword i32) (local $ws i32)
    (local $len i32) (local $tmp i32) (local $k i32)
    (local.set $i (call $find (local.get $p) (local.get $n)))
    (if (i32.ge_s (local.get $i) (i32.const 0))
      (then
        (local.set $i (i32.add (local.get $i) (i32.const 8)))
        (block $end
          (loop $scan
            (br_if $end (i32.ge_u (local.get $i) (local.get $n)))
            (local.set $c (i32.load8_u (i32.add (local.get $p) (local.get $i))))
            (br_if $end (i32.eq (local.get $c) (i32.const 34)))          ;; closing "
            (local.set $ws (i32.le_u (local.get $c) (i32.const 32)))
            (if (i32.eq (local.get $c) (i32.const 92))                    ;; \ escape
              (then
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (local.set $c (i32.load8_u (i32.add (local.get $p) (local.get $i))))
                ;; \n and \t separate words
                (local.set $ws (i32.or (i32.eq (local.get $c) (i32.const 110))
                                       (i32.eq (local.get $c) (i32.const 116))))))
            (if (local.get $ws)
              (then (local.set $inword (i32.const 0)))
              (else
                (if (i32.eqz (local.get $inword))
                  (then (local.set $words (i32.add (local.get $words) (i32.const 1)))))
                (local.set $inword (i32.const 1))))
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br $scan)))))
    ;; Build {"message":"N words"} at address 1024.
    (memory.copy (i32.const 1024) (i32.const 16) (i32.const 12))
    (local.set $len (i32.const 12))
    (local.set $tmp (local.get $words))
    (loop $digits                                     ;; reversed, at 2000
      (i32.store8 (i32.add (i32.const 2000) (local.get $k))
                  (i32.add (i32.const 48) (i32.rem_u (local.get $tmp) (i32.const 10))))
      (local.set $k (i32.add (local.get $k) (i32.const 1)))
      (local.set $tmp (i32.div_u (local.get $tmp) (i32.const 10)))
      (br_if $digits (i32.ne (local.get $tmp) (i32.const 0))))
    (loop $copy
      (local.set $k (i32.sub (local.get $k) (i32.const 1)))
      (i32.store8 (i32.add (i32.const 1024) (local.get $len))
                  (i32.load8_u (i32.add (i32.const 2000) (local.get $k))))
      (local.set $len (i32.add (local.get $len) (i32.const 1)))
      (br_if $copy (i32.ne (local.get $k) (i32.const 0))))
    (memory.copy (i32.add (i32.const 1024) (local.get $len)) (i32.const 32) (i32.const 8))
    (local.set $len (i32.add (local.get $len) (i32.const 8)))
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.extend_i32_u (local.get $len)))))
