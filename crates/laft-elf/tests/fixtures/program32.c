/* A 32-bit dynamic executable that imports from libversioned.so.1 without libc. */

extern int add(int, int, int);

void _start(void) {
  int status = add(1, 2, 3);
  __asm__ volatile("int $0x80" : : "a"(1), "b"(status) : "memory");
  __builtin_unreachable();
}
