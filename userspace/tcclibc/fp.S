// sqrt and rounding: one instruction each (see math.h). tcc's assembler
// can't write these, so each is spelled as its encoding, all on d0.

.text
.global sqrt
sqrt:
    .int 0x1e61c000  // fsqrt d0, d0
    ret

.global floor
floor:
    .int 0x1e654000  // frintm d0, d0 (towards -infinity)
    ret

.global ceil
ceil:
    .int 0x1e64c000  // frintp d0, d0 (towards +infinity)
    ret

.global round
round:
    .int 0x1e664000  // frinta d0, d0 (to nearest, halves away from 0)
    ret

.global trunc
trunc:
    .int 0x1e65c000  // frintz d0, d0 (towards 0)
    ret
