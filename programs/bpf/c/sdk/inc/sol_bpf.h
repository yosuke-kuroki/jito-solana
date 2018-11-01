#pragma once
/**
 * @brief Solana C-based BPF program utility functions and types
 */

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Numeric types
 */
#ifndef __LP64__
#error LP64 data model required
#endif

typedef signed char int8_t;
typedef unsigned char uint8_t;
typedef signed short int16_t;
typedef unsigned short uint16_t;
typedef signed int int32_t;
typedef unsigned int uint32_t;
typedef signed long int int64_t;
typedef unsigned long int uint64_t;

/**
 * NULL
 */
#define NULL 0

/**
 * Boolean type
 */
typedef enum { false = 0, true } bool;

/**
 * Built-in helper functions
 * @{
 * The BPF VM makes a limited number of helper functions available to BPF
 * programs.  They are resolved at run-time and identified by a function index.
 * Calling any of these functions results in `Call` instruction out of the
 * user's BPF program.
 *
 * The helper functions all follow the same signature:
 *
 * int helper(uint64_t, uint64_t, uint64_t, uint64_t, uint64_t)
 *
 * The meaning of each argument and return value is dependent on the particular
 * helper function being called.
 */

/**
 * Helper function that prints to stdout
 *
 * Prints the hexadecimal representation of each parameter
 */
#define BPF_TRACE_PRINTK_IDX 6
static int (*sol_print)(
  uint64_t,
  uint64_t,
  uint64_t,
  uint64_t,
  uint64_t
) = (void *)BPF_TRACE_PRINTK_IDX;

/**@}*/

/**
 * Prefix for all BPF functions
 *
 * This prefix should be used for functions in order to facilitate
 * interoperability with BPF representation
 */
#define SOL_FN_PREFIX __attribute__((always_inline)) static

/**
 * Size of Public key in bytes
 */
#define SIZE_PUBKEY 32

/**
 * Public key
 */
typedef struct {
  uint8_t x[SIZE_PUBKEY];
} SolPubkey;

/**
 * Compares two public keys
 *
 * @param one First public key
 * @param two Second public key
 * @return true if the same
 */
SOL_FN_PREFIX bool SolPubkey_same(const SolPubkey *one, const SolPubkey *two) {
  for (int i = 0; i < sizeof(*one); i++) {
    if (one->x[i] != two->x[i]) {
      return false;
    }
  }
  return true;
}

/**
 * Keyed Accounts
 */
typedef struct {
  SolPubkey *key;        /** Public Key of the account owner */
  int64_t *tokens;       /** Numer of tokens owned by this account */
  uint64_t userdata_len; /** Length of userdata in bytes */
  uint8_t *userdata;     /** On-chain data owned by this account */
  SolPubkey *program_id; /** Program that owns this account */
} SolKeyedAccounts;

/**
 * Copies memory
 */
SOL_FN_PREFIX void sol_memcpy(void *dst, const void *src, int len) {
  for (int i = 0; i < len; i++) {
    *((uint8_t *)dst + i) = *((const uint8_t *)src + i);
  }
}

/**
 * Compares memory
 */
SOL_FN_PREFIX int sol_memcmp(const void *s1, const void *s2, int n) {
  for (int i = 0; i < n; i++) {
    uint8_t diff = *((uint8_t *)s1 + i) - *((const uint8_t *)s2 + i);
    if (diff) {
      return diff;
    }
  }
  return 0;
}

/**
 * Computes the number of elements in an array
 */
#define SOL_ARRAY_SIZE(a) (sizeof(a) / sizeof(a[0]))

/**
 * Panics
 *
 * Prints the line number where the panic occurred and then causes
 * the BPF VM to immediately halt execution. No accounts' userdata are updated
 */
#define sol_panic() _sol_panic(__LINE__)
SOL_FN_PREFIX void _sol_panic(uint64_t line) {
  sol_print(0xFF, 0xFF, 0xFF, 0xFF, line);
  uint8_t *pv = (uint8_t *)1;
  *pv = 1;
}

/**
 * Asserts
 */
#define sol_assert(expr)  \
  if (!(expr)) {          \
    _sol_panic(__LINE__); \
  }

/**
 * De-serializes the input parameters into usable types
 *
 * Use this function to deserialize the buffer passed to the program entrypoint
 * into usable types.  This function does not perform copy deserialization,
 * instead it populates the pointers and lengths in SolKeyedAccounts and data so
 * that any modification to tokens or account data take place on the original
 * buffer.  Doing so also eliminates the need to serialize back into the buffer
 * at program end.
 *
 * @param input Source buffer containing serialized input parameters
 * @param ka Pointer to an array of SolKeyedAccounts to deserialize into
 * @param ka_len Number of SolKeyedAccounts entries in `ka`
 * @param ka_len_out If NULL, fill exactly `ka_len` accounts or fail.
 *                   If not NULL, fill up to `ka_len` accounts and return the
 *                   number of filled accounts in `ka_len_out`.
 * @param data On return, a pointer to the instruction data
 * @param data_len On return, the length in bytes of the instruction data
 * @return Boolean true if successful
 */
SOL_FN_PREFIX bool sol_deserialize(
  const uint8_t *input,
  SolKeyedAccounts *ka,
  uint64_t ka_len,
  uint64_t *ka_len_out,
  const uint8_t **data,
  uint64_t *data_len
) {


  if (ka_len_out == NULL) {
    if (ka_len != *(uint64_t *) input) {
      return false;
    }
    ka_len = *(uint64_t *) input;
  } else {
    if (ka_len > *(uint64_t *) input) {
      ka_len = *(uint64_t *) input;
    }
    *ka_len_out = ka_len;
  }

  input += sizeof(uint64_t);
  for (int i = 0; i < ka_len; i++) {
    // key
    ka[i].key = (SolPubkey *) input;
    input += sizeof(SolPubkey);

    // tokens
    ka[i].tokens = (int64_t *) input;
    input += sizeof(int64_t);

    // account userdata
    ka[i].userdata_len = *(uint64_t *) input;
    input += sizeof(uint64_t);
    ka[i].userdata = input;
    input += ka[i].userdata_len;

    // program_id
    ka[i].program_id = (SolPubkey *) input;
    input += sizeof(SolPubkey);
  }

  // input data
  *data_len = *(uint64_t *) input;
  input += sizeof(uint64_t);
  *data = input;

  return true;
}

/**
 * Debugging utilities
 * @{
 */

/**
 * Prints the hexadecimal representation of a public key
 *
 * @param key The public key to print
 */
SOL_FN_PREFIX void sol_print_key(const SolPubkey *key) {
  for (int j = 0; j < sizeof(*key); j++) {
    sol_print(0, 0, 0, j, key->x[j]);
  }
}

/**
 * Prints the hexadecimal representation of an array
 *
 * @param array The array to print
 */
SOL_FN_PREFIX void sol_print_array(const uint8_t *array, int len) {
  for (int j = 0; j < len; j++) {
    sol_print(0, 0, 0, j, array[j]);
  }
}

/**
 * Prints the hexadecimal representation of the program's input parameters
 *
 * @param num_ka Numer of SolKeyedAccounts to print
 * @param ka A pointer to an array of SolKeyedAccounts to print
 * @param data A pointer to the instruction data to print
 * @param data_len The length in bytes of the instruction data
 */
SOL_FN_PREFIX void sol_print_params(
  uint64_t num_ka,
  const SolKeyedAccounts *ka,
  const uint8_t *data,
  uint64_t data_len
) {
  sol_print(0, 0, 0, 0, num_ka);
  for (int i = 0; i < num_ka; i++) {
    sol_print_key(ka[i].key);
    sol_print(0, 0, 0, 0, *ka[i].tokens);
    sol_print_array(ka[i].userdata, ka[i].userdata_len);
    sol_print_key(ka[i].program_id);
  }
  sol_print_array(data, data_len);
}

/**@}*/

/**
 * Program entrypoint
 * @{
 *
 * The following is an example of a simple program that prints the input
 * parameters it received:
 *
 * bool entrypoint(const uint8_t *input) {
 *   SolKeyedAccounts ka[1];
 *   uint8_t *data;
 *   uint64_t data_len;
 *
 *   if (!sol_deserialize(buf, ka, SOL_ARRAY_SIZE(ka), NULL, &data, &data_len)) {
 *     return false;
 *   }
 *   print_params(1, ka, data, data_len);
 *   return true;
 * }
 */

/**
 * Program entrypoint signature
 *
 * @param input An array containing serialized input parameters
 * @return true if successful
 */
extern bool entrypoint(const uint8_t *input);

#ifdef __cplusplus
}
#endif

/**@}*/
