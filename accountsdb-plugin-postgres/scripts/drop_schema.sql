/**
 * Script for cleaning up the schema for PostgreSQL used for the AccountsDb plugin.
 */

DROP TRIGGER account_update_trigger ON account;
DROP FUNCTION audit_account_update;
DROP TABLE account_audit;
DROP TABLE account;
DROP TABLE slot;
DROP TABLE transaction;

DROP TYPE "TransactionError" CASCADE;
DROP TYPE "TransactionErrorCode" CASCADE;
DROP TYPE "MappedMessage" CASCADE;
DROP TYPE "MappedAddresses" CASCADE;
DROP TYPE "TransactionMessageV0" CASCADE;
DROP TYPE "AddressMapIndexes" CASCADE;
DROP TYPE "TransactionMessage" CASCADE;
DROP TYPE "TransactionMessageHeader" CASCADE;
DROP TYPE "TransactionStatusMeta" CASCADE;
DROP TYPE "RewardType" CASCADE;
DROP TYPE "Reward" CASCADE;
DROP TYPE "TransactionTokenBalance" CASCADE;
DROP TYPE "InnerInstructions" CASCADE;
DROP TYPE "CompiledInstruction" CASCADE;
