;;! target = "x86_64"
;;! test = "compile"
;;! flags = " -C cranelift-enable-heap-access-spectre-mitigation -W memory64 -O static-memory-maximum-size=0 -O static-memory-guard-size=4294967295 -O dynamic-memory-guard-size=4294967295"

;; !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
;; !!! GENERATED BY 'make-load-store-tests.sh' DO NOT EDIT !!!
;; !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!

(module
  (memory i64 1)

  (func (export "do_store") (param i64 i32)
    local.get 0
    local.get 1
    i32.store8 offset=0)

  (func (export "do_load") (param i64) (result i32)
    local.get 0
    i32.load8_u offset=0))

;; function u0:0:
;;   pushq   %rbp
;;   unwind PushFrameRegs { offset_upward_to_caller_sp: 16 }
;;   movq    %rsp, %rbp
;;   unwind DefineNewFrame { offset_upward_to_caller_sp: 16, offset_downward_to_clobbers: 0 }
;; block0:
;;   movq    88(%rdi), %rsi
;;   xorq    %r11, %r11, %r11
;;   movq    %rdx, %r10
;;   addq    %r10, 80(%rdi), %r10
;;   cmpq    %rsi, %rdx
;;   cmovnbq %r11, %r10, %r10
;;   movb    %cl, 0(%r10)
;;   jmp     label1
;; block1:
;;   movq    %rbp, %rsp
;;   popq    %rbp
;;   ret
;;
;; function u0:1:
;;   pushq   %rbp
;;   unwind PushFrameRegs { offset_upward_to_caller_sp: 16 }
;;   movq    %rsp, %rbp
;;   unwind DefineNewFrame { offset_upward_to_caller_sp: 16, offset_downward_to_clobbers: 0 }
;; block0:
;;   movq    88(%rdi), %rsi
;;   xorq    %r11, %r11, %r11
;;   movq    %rdx, %r10
;;   addq    %r10, 80(%rdi), %r10
;;   cmpq    %rsi, %rdx
;;   cmovnbq %r11, %r10, %r10
;;   movzbq  0(%r10), %rax
;;   jmp     label1
;; block1:
;;   movq    %rbp, %rsp
;;   popq    %rbp
;;   ret