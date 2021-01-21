/**
 * @brief Instruction definitions for the invoked program
 */

#include <solana_sdk.h>

const uint8_t VERIFY_TRANSLATIONS = 0;
const uint8_t RETURN_ERROR = 1;
const uint8_t DERIVED_SIGNERS = 2;
const uint8_t VERIFY_NESTED_SIGNERS = 3;
const uint8_t VERIFY_WRITER = 4;
const uint8_t VERIFY_PRIVILEGE_ESCALATION = 5;
const uint8_t NESTED_INVOKE = 6;
const uint8_t RETURN_OK = 7;
const uint8_t VERIFY_PRIVILEGE_DEESCALATION = 8;
const uint8_t VERIFY_PRIVILEGE_DEESCALATION_ESCALATION_SIGNER = 9;
const uint8_t VERIFY_PRIVILEGE_DEESCALATION_ESCALATION_WRITABLE = 10;
