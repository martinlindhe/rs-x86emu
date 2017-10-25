    org  0x100        ; .com files always start 256 bytes into the segment

section .data
    %include "data.inc.asm"

section .bss
    ; uninitialized data


section .text
    ; program code
start:
    call clear_regs

    ; ------------------
    ; run a instruction
    mov ax, 0x30
    mov bl, 2
    idiv bl   ; 0x30 / 2 = 0x15 .... ax = 0x0018 in dosbox


    ; save reg states after instruction executes
    mov [_ax], ax
    mov [_bx], bx
    mov [_cx], cx
    mov [_dx], dx
    mov [_sp], sp
    mov [_bp], bp
    mov [_si], si
    mov [_di], di

    mov [_es], es
    mov [_cs], cs
    mov [_ss], ss
    mov [_ds], ds
    mov [_fs], fs
    mov [_gs], gs

    ; read FLAGS 16bit reg
    pushf
    pop ax
    mov [_flags], ax


    call print_regs


    call test_instr

    mov  ah, 0x4c       ; exit to dos
    int  0x21

; tests instructions for correct emulation
test_instr:
    mov ax, 0x30
    mov bl, 2
    idiv bl   ; ax = 0x0018 in dosbox, XXX test on real hw
    cmp ax, 0x0018
    je t2
    mov dx, test1fail
    call print_dollar_dx

t2:
    ret
%include "regs.inc.asm"
%include "print.inc.asm"
