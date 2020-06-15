// @flow

import {Connection} from '../connection';
import {Transaction} from '../transaction';
import {sleep} from './sleep';
import type {Account} from '../account';

/**
 * Sign, send and confirm a transaction
 */
export async function sendAndConfirmTransaction(
  connection: Connection,
  from: Account,
  transaction: Transaction,
  runtimeErrorOk: boolean = false
): Promise<void> {

  let sendRetries = 3;
  for (;;) {
    const start = Date.now();
    const signature = await connection.sendTransaction(from, transaction);

    // Wait up to a couple seconds for a confirmation
    let status = 'SignatureNotFound';
    let statusRetries = 4;
    for (;;) {
      status = await connection.getSignatureStatus(signature);
      if (status !== 'SignatureNotFound') {
        break;
      }

      await sleep(500);
      if (--statusRetries <= 0) {
        const duration = (Date.now() - start) / 1000;
        throw new Error(`Transaction '${signature}' was not confirmed in ${duration.toFixed(2)} seconds (${status})`);
      }
    }

    if ( (status === 'Confirmed') ||
         (status === 'ProgramRuntimeError' && runtimeErrorOk) ) {
      return;
    }

    if (status !== 'AccountInUse' || --sendRetries <= 0) {
      throw new Error(`Transaction ${signature} failed (${status})`);
    }
  }
}

