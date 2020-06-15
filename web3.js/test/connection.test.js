// @flow
import {
  Account,
  Connection,
  SystemProgram,
  sendAndConfirmTransaction,
  SOL_LAMPORTS,
} from '../src';
import {DEFAULT_TICKS_PER_SLOT, NUM_TICKS_PER_SECOND} from '../src/timing';
import {mockRpc, mockRpcEnabled} from './__mocks__/node-fetch';
import {mockGetRecentBlockhash} from './mockrpc/get-recent-blockhash';
import {url} from './url';
import {sleep} from '../src/util/sleep';

if (!mockRpcEnabled) {
  // Testing max commitment level takes around 20s to complete
  jest.setTimeout(30000);
}

const errorMessage = 'Invalid request';
const errorResponse = {
  error: {
    message: errorMessage,
  },
  result: undefined,
};

test('get account info - error', () => {
  const account = new Account();
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getAccountInfo',
      params: [account.publicKey.toBase58()],
    },
    errorResponse,
  ]);

  return expect(connection.getAccountInfo(account.publicKey)).rejects.toThrow(
    errorMessage,
  );
});

test('get program accounts', async () => {
  if (mockRpcEnabled) {
    console.log('non-live test skipped');
    return;
  }

  const connection = new Connection(url, 'recent');
  const account0 = new Account();
  const account1 = new Account();
  const programId = new Account();
  await connection.requestAirdrop(account0.publicKey, SOL_LAMPORTS);
  await connection.requestAirdrop(account1.publicKey, 0.5 * SOL_LAMPORTS);

  let transaction = SystemProgram.assign(
    account0.publicKey,
    programId.publicKey,
  );
  await sendAndConfirmTransaction(connection, transaction, account0);

  transaction = SystemProgram.assign(account1.publicKey, programId.publicKey);
  await sendAndConfirmTransaction(connection, transaction, account1);

  const [, feeCalculator] = await connection.getRecentBlockhash();

  const programAccounts = await connection.getProgramAccounts(
    programId.publicKey,
  );
  expect(programAccounts.length).toBe(2);

  programAccounts.forEach(function(element) {
    expect([
      account0.publicKey.toBase58(),
      account1.publicKey.toBase58(),
    ]).toEqual(expect.arrayContaining([element[0]]));
    if (element[0] == account0.publicKey) {
      expect(element[1].lamports).toBe(
        SOL_LAMPORTS - feeCalculator.lamportsPerSignature,
      );
    } else {
      expect(element[1].lamports).toBe(
        0.5 * SOL_LAMPORTS - feeCalculator.lamportsPerSignature,
      );
    }
  });
});

test('validatorExit', async () => {
  if (!mockRpcEnabled) {
    console.log('validatorExit skipped on live node');
    return;
  }
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'validatorExit',
    },
    {
      error: null,
      result: false,
    },
  ]);

  const result = await connection.validatorExit();
  expect(result).toBe(false);
});

test('get balance', async () => {
  const account = new Account();
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getBalance',
      params: [account.publicKey.toBase58()],
    },
    {
      error: null,
      result: 0,
    },
  ]);

  const balance = await connection.getBalance(account.publicKey);
  expect(balance).toBeGreaterThanOrEqual(0);
});

test('get inflation', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getInflation',
      params: [],
    },
    {
      error: null,
      result: {
        foundation: 0.05,
        foundation_term: 7.0,
        initial: 0.15,
        storage: 0.1,
        taper: 0.15,
        terminal: 0.015,
      },
    },
  ]);

  const inflation = await connection.getInflation();

  for (const key of [
    'initial',
    'terminal',
    'taper',
    'foundation',
    'foundation_term',
    'storage',
  ]) {
    expect(inflation).toHaveProperty(key);
    expect(inflation[key]).toBeGreaterThan(0);
  }
});

test('get epoch info', async () => {
  const connection = new Connection(url, 'recent');

  mockRpc.push([
    url,
    {
      method: 'getEpochInfo',
      params: [{commitment: 'recent'}],
    },
    {
      error: null,
      result: {
        epoch: 0,
        slotIndex: 1,
        slotsInEpoch: 8192,
        absoluteSlot: 1,
      },
    },
  ]);

  const epochInfo = await connection.getEpochInfo();

  for (const key of ['epoch', 'slotIndex', 'slotsInEpoch', 'absoluteSlot']) {
    expect(epochInfo).toHaveProperty(key);
    expect(epochInfo[key]).toBeGreaterThanOrEqual(0);
  }
});

test('get epoch schedule', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getEpochSchedule',
      params: [],
    },
    {
      error: null,
      result: {
        first_normal_epoch: 8,
        first_normal_slot: 8160,
        leader_schedule_slot_offset: 8192,
        slots_per_epoch: 8192,
        warmup: true,
      },
    },
  ]);

  const epochSchedule = await connection.getEpochSchedule();

  for (const key of [
    'first_normal_epoch',
    'first_normal_slot',
    'leader_schedule_slot_offset',
    'slots_per_epoch',
  ]) {
    expect(epochSchedule).toHaveProperty(key);
    expect(epochSchedule[key]).toBeGreaterThan(0);
  }

  expect(epochSchedule).toHaveProperty('warmup');
  expect(epochSchedule.warmup).toBeTruthy();
});

test('get slot', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getSlot',
    },
    {
      error: null,
      result: 123,
    },
  ]);

  const slotLeader = await connection.getSlot();
  if (mockRpcEnabled) {
    expect(slotLeader).toBe(123);
  } else {
    // No idea what the correct slot value should be on a live cluster, so
    // just check the type
    expect(typeof slotLeader).toBe('number');
  }
});

test('get slot leader', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getSlotLeader',
    },
    {
      error: null,
      result: '11111111111111111111111111111111',
    },
  ]);

  const slotLeader = await connection.getSlotLeader();
  if (mockRpcEnabled) {
    expect(slotLeader).toBe('11111111111111111111111111111111');
  } else {
    // No idea what the correct slotLeader value should be on a live cluster, so
    // just check the type
    expect(typeof slotLeader).toBe('string');
  }
});

test('get cluster nodes', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getClusterNodes',
    },
    {
      error: null,
      result: [
        {
          pubkey: '11111111111111111111111111111111',
          gossip: '127.0.0.0:1234',
          tpu: '127.0.0.0:1235',
          rpc: null,
        },
      ],
    },
  ]);

  const clusterNodes = await connection.getClusterNodes();
  if (mockRpcEnabled) {
    expect(clusterNodes).toHaveLength(1);
    expect(clusterNodes[0].pubkey).toBe('11111111111111111111111111111111');
    expect(typeof clusterNodes[0].gossip).toBe('string');
    expect(typeof clusterNodes[0].tpu).toBe('string');
    expect(clusterNodes[0].rpc).toBeNull();
  } else {
    // There should be at least one node (the node that we're talking to)
    expect(clusterNodes.length).toBeGreaterThan(0);
  }
});

test('getVoteAccounts', async () => {
  if (mockRpcEnabled) {
    console.log('non-live test skipped');
    return;
  }

  const connection = new Connection(url);
  const voteAccounts = await connection.getVoteAccounts();
  expect(
    voteAccounts.current.concat(voteAccounts.delinquent).length,
  ).toBeGreaterThan(0);
});

test('confirm transaction - error', async () => {
  const connection = new Connection(url);

  const badTransactionSignature = 'bad transaction signature';

  mockRpc.push([
    url,
    {
      method: 'confirmTransaction',
      params: [badTransactionSignature],
    },
    errorResponse,
  ]);

  await expect(
    connection.confirmTransaction(badTransactionSignature),
  ).rejects.toThrow(errorMessage);

  mockRpc.push([
    url,
    {
      method: 'getSignatureStatus',
      params: [badTransactionSignature],
    },
    errorResponse,
  ]);

  await expect(
    connection.getSignatureStatus(badTransactionSignature),
  ).rejects.toThrow(errorMessage);
});

test('get transaction count', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getTransactionCount',
      params: [],
    },
    {
      error: null,
      result: 1000000,
    },
  ]);

  const count = await connection.getTransactionCount();
  expect(count).toBeGreaterThanOrEqual(0);
});

test('get total supply', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getTotalSupply',
      params: [],
    },
    {
      error: null,
      result: 1000000,
    },
  ]);

  const count = await connection.getTotalSupply();
  expect(count).toBeGreaterThanOrEqual(0);
});

test('get minimum balance for rent exemption', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getMinimumBalanceForRentExemption',
      params: [512],
    },
    {
      error: null,
      result: 1000000,
    },
  ]);

  const count = await connection.getMinimumBalanceForRentExemption(512);
  expect(count).toBeGreaterThanOrEqual(0);
});

test('get confirmed block', async () => {
  if (mockRpcEnabled) {
    console.log('non-live test skipped');
    return;
  }
  const connection = new Connection(url);

  while ((await connection.getSlot()) <= 0) {
    continue;
  }

  // Block 0 never has any transactions in automation localnet
  const block0 = await connection.getConfirmedBlock(0);
  const blockhash0 = block0.blockhash;
  expect(block0.transactions.length).toBe(0);
  expect(blockhash0).not.toBeNull();
  expect(block0.previousBlockhash).not.toBeNull();
  expect(block0.parentSlot).toBe(0);

  // Find a block that has a transaction, usually Block 1
  let x = 1;
  while (x < 10) {
    const block1 = await connection.getConfirmedBlock(x);
    if (block1.transactions.length >= 1) {
      expect(block1.previousBlockhash).toBe(blockhash0);
      expect(block1.blockhash).not.toBeNull();
      expect(block1.parentSlot).toBe(0);
      break;
    }
    x++;
  }
});

test('get recent blockhash', async () => {
  const connection = new Connection(url);

  mockGetRecentBlockhash();

  const [
    recentBlockhash,
    feeCalculator,
  ] = await connection.getRecentBlockhash();
  expect(recentBlockhash.length).toBeGreaterThanOrEqual(43);
  expect(feeCalculator.lamportsPerSignature).toBeGreaterThanOrEqual(0);
});

test('getVersion', async () => {
  const connection = new Connection(url);

  mockRpc.push([
    url,
    {
      method: 'getVersion',
      params: [],
    },
    {
      error: null,
      result: {'solana-core': '0.20.4'},
    },
  ]);

  const version = await connection.getVersion();
  expect(version['solana-core']).toBeTruthy();
});

test('request airdrop', async () => {
  const account = new Account();
  const connection = new Connection(url, 'recent');

  mockRpc.push([
    url,
    {
      method: 'requestAirdrop',
      params: [account.publicKey.toBase58(), 40, {commitment: 'recent'}],
    },
    {
      error: null,
      result:
        '1WE5w4B7v59x6qjyC4FbG2FEKYKQfvsJwqSxNVmtMjT8TQ31hsZieDHcSgqzxiAoTL56n2w5TncjqEKjLhtF4Vk',
    },
  ]);
  mockRpc.push([
    url,
    {
      method: 'requestAirdrop',
      params: [account.publicKey.toBase58(), 2, {commitment: 'recent'}],
    },
    {
      error: null,
      result:
        '2WE5w4B7v59x6qjyC4FbG2FEKYKQfvsJwqSxNVmtMjT8TQ31hsZieDHcSgqzxiAoTL56n2w5TncjqEKjLhtF4Vk',
    },
  ]);
  mockRpc.push([
    url,
    {
      method: 'getBalance',
      params: [account.publicKey.toBase58(), {commitment: 'recent'}],
    },
    {
      error: null,
      result: 42,
    },
  ]);

  await connection.requestAirdrop(account.publicKey, 40);
  await connection.requestAirdrop(account.publicKey, 2);

  const balance = await connection.getBalance(account.publicKey);
  expect(balance).toBe(42);

  mockRpc.push([
    url,
    {
      method: 'getAccountInfo',
      params: [account.publicKey.toBase58(), {commitment: 'recent'}],
    },
    {
      error: null,
      result: {
        owner: [
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0,
        ],
        lamports: 42,
        data: [],
        executable: false,
      },
    },
  ]);

  const accountInfo = await connection.getAccountInfo(account.publicKey);
  expect(accountInfo.lamports).toBe(42);
  expect(accountInfo.data).toHaveLength(0);
  expect(accountInfo.owner).toEqual(SystemProgram.programId);
});

// expected to take around 20s
test('request airdrop - max commitment', async () => {
  const account = new Account();
  const connection = new Connection(url, 'max');

  mockRpc.push([
    url,
    {
      method: 'requestAirdrop',
      params: [account.publicKey.toBase58(), 40, {commitment: 'max'}],
    },
    {
      error: null,
      result:
        '1WE5w4B7v59x6qjyC4FbG2FEKYKQfvsJwqSxNVmtMjT8TQ31hsZieDHcSgqzxiAoTL56n2w5TncjqEKjLhtF4Vk',
    },
  ]);
  mockRpc.push([
    url,
    {
      method: 'getBalance',
      params: [account.publicKey.toBase58(), {commitment: 'max'}],
    },
    {
      error: null,
      result: 40,
    },
  ]);

  await connection.requestAirdrop(account.publicKey, 40);
  const balance = await connection.getBalance(account.publicKey);
  expect(balance).toBe(40);
});

test('transaction', async () => {
  const accountFrom = new Account();
  const accountTo = new Account();
  const connection = new Connection(url, 'recent');

  mockRpc.push([
    url,
    {
      method: 'requestAirdrop',
      params: [
        accountFrom.publicKey.toBase58(),
        100010,
        {commitment: 'recent'},
      ],
    },
    {
      error: null,
      result:
        '0WE5w4B7v59x6qjyC4FbG2FEKYKQfvsJwqSxNVmtMjT8TQ31hsZieDHcSgqzxiAoTL56n2w5TncjqEKjLhtF4Vk',
    },
  ]);
  mockRpc.push([
    url,
    {
      method: 'getBalance',
      params: [accountFrom.publicKey.toBase58(), {commitment: 'recent'}],
    },
    {
      error: null,
      result: 100010,
    },
  ]);
  await connection.requestAirdrop(accountFrom.publicKey, 100010);
  expect(await connection.getBalance(accountFrom.publicKey)).toBe(100010);

  mockRpc.push([
    url,
    {
      method: 'requestAirdrop',
      params: [accountTo.publicKey.toBase58(), 21, {commitment: 'recent'}],
    },
    {
      error: null,
      result:
        '8WE5w4B7v59x6qjyC4FbG2FEKYKQfvsJwqSxNVmtMjT8TQ31hsZieDHcSgqzxiAoTL56n2w5TncjqEKjLhtF4Vk',
    },
  ]);
  mockRpc.push([
    url,
    {
      method: 'getBalance',
      params: [accountTo.publicKey.toBase58(), {commitment: 'recent'}],
    },
    {
      error: null,
      result: 21,
    },
  ]);
  await connection.requestAirdrop(accountTo.publicKey, 21);
  expect(await connection.getBalance(accountTo.publicKey)).toBe(21);

  mockGetRecentBlockhash('recent');
  mockRpc.push([
    url,
    {
      method: 'sendTransaction',
    },
    {
      error: null,
      result:
        '3WE5w4B7v59x6qjyC4FbG2FEKYKQfvsJwqSxNVmtMjT8TQ31hsZieDHcSgqzxiAoTL56n2w5TncjqEKjLhtF4Vk',
    },
  ]);

  const transaction = SystemProgram.transfer(
    accountFrom.publicKey,
    accountTo.publicKey,
    10,
  );
  const signature = await connection.sendTransaction(transaction, accountFrom);

  mockRpc.push([
    url,
    {
      method: 'confirmTransaction',
      params: [
        '3WE5w4B7v59x6qjyC4FbG2FEKYKQfvsJwqSxNVmtMjT8TQ31hsZieDHcSgqzxiAoTL56n2w5TncjqEKjLhtF4Vk',
        {commitment: 'recent'},
      ],
    },
    {
      error: null,
      result: true,
    },
  ]);

  let i = 0;
  for (;;) {
    if (await connection.confirmTransaction(signature)) {
      break;
    }
    console.log('not confirmed', signature);
    expect(mockRpcEnabled).toBe(false);
    expect(++i).toBeLessThan(10);
    await sleep(500);
  }

  mockRpc.push([
    url,
    {
      method: 'getSignatureStatus',
      params: [
        '3WE5w4B7v59x6qjyC4FbG2FEKYKQfvsJwqSxNVmtMjT8TQ31hsZieDHcSgqzxiAoTL56n2w5TncjqEKjLhtF4Vk',
        {commitment: 'recent'},
      ],
    },
    {
      error: null,
      result: {Ok: null},
    },
  ]);
  await expect(connection.getSignatureStatus(signature)).resolves.toEqual({
    Ok: null,
  });

  mockRpc.push([
    url,
    {
      method: 'getBalance',
      params: [accountFrom.publicKey.toBase58(), {commitment: 'recent'}],
    },
    {
      error: null,
      result: 2,
    },
  ]);

  // accountFrom may have less than 100000 due to transaction fees
  const balance = await connection.getBalance(accountFrom.publicKey);
  expect(balance).toBeGreaterThan(0);
  expect(balance).toBeLessThanOrEqual(100000);

  mockRpc.push([
    url,
    {
      method: 'getBalance',
      params: [accountTo.publicKey.toBase58(), {commitment: 'recent'}],
    },
    {
      error: null,
      result: 31,
    },
  ]);
  expect(await connection.getBalance(accountTo.publicKey)).toBe(31);
});

test('multi-instruction transaction', async () => {
  if (mockRpcEnabled) {
    console.log('non-live test skipped');
    return;
  }

  const accountFrom = new Account();
  const accountTo = new Account();
  const connection = new Connection(url, 'recent');

  await connection.requestAirdrop(accountFrom.publicKey, SOL_LAMPORTS);
  expect(await connection.getBalance(accountFrom.publicKey)).toBe(SOL_LAMPORTS);

  await connection.requestAirdrop(accountTo.publicKey, 21);
  expect(await connection.getBalance(accountTo.publicKey)).toBe(21);

  // 1. Move(accountFrom, accountTo)
  // 2. Move(accountTo, accountFrom)
  const transaction = SystemProgram.transfer(
    accountFrom.publicKey,
    accountTo.publicKey,
    100,
  ).add(
    SystemProgram.transfer(accountTo.publicKey, accountFrom.publicKey, 100),
  );
  const signature = await connection.sendTransaction(
    transaction,
    accountFrom,
    accountTo,
  );
  let i = 0;
  for (;;) {
    if (await connection.confirmTransaction(signature)) {
      break;
    }

    expect(mockRpcEnabled).toBe(false);
    expect(++i).toBeLessThan(10);
    await sleep(500);
  }
  await expect(connection.getSignatureStatus(signature)).resolves.toEqual({
    Ok: null,
  });

  // accountFrom may have less than SOL_LAMPORTS due to transaction fees
  expect(await connection.getBalance(accountFrom.publicKey)).toBeGreaterThan(0);
  expect(
    await connection.getBalance(accountFrom.publicKey),
  ).toBeLessThanOrEqual(SOL_LAMPORTS);

  expect(await connection.getBalance(accountTo.publicKey)).toBe(21);
});

test('account change notification', async () => {
  if (mockRpcEnabled) {
    console.log('non-live test skipped');
    return;
  }

  const connection = new Connection(url, 'recent');
  const owner = new Account();
  const programAccount = new Account();

  const mockCallback = jest.fn();

  const subscriptionId = connection.onAccountChange(
    programAccount.publicKey,
    mockCallback,
  );

  const balanceNeeded = Math.max(
    await connection.getMinimumBalanceForRentExemption(0),
    1,
  );

  await connection.requestAirdrop(owner.publicKey, SOL_LAMPORTS);
  try {
    let transaction = SystemProgram.transfer(
      owner.publicKey,
      programAccount.publicKey,
      balanceNeeded,
    );
    await sendAndConfirmTransaction(connection, transaction, owner);
  } catch (err) {
    await connection.removeAccountChangeListener(subscriptionId);
    throw err;
  }

  // Wait for mockCallback to receive a call
  let i = 0;
  for (;;) {
    if (mockCallback.mock.calls.length > 0) {
      break;
    }

    if (++i === 30) {
      throw new Error('Account change notification not observed');
    }
    // Sleep for a 1/4 of a slot, notifications only occur after a block is
    // processed
    await sleep((250 * DEFAULT_TICKS_PER_SLOT) / NUM_TICKS_PER_SECOND);
  }

  await connection.removeAccountChangeListener(subscriptionId);

  expect(mockCallback.mock.calls[0][0].lamports).toBe(balanceNeeded);
  expect(mockCallback.mock.calls[0][0].owner).toEqual(SystemProgram.programId);
});

test('program account change notification', async () => {
  if (mockRpcEnabled) {
    console.log('non-live test skipped');
    return;
  }

  const connection = new Connection(url, 'recent');
  const owner = new Account();
  const programAccount = new Account();

  // const mockCallback = jest.fn();

  const balanceNeeded = await connection.getMinimumBalanceForRentExemption(0);

  let notified = false;
  const subscriptionId = connection.onProgramAccountChange(
    SystemProgram.programId,
    keyedAccountInfo => {
      if (keyedAccountInfo.accountId !== programAccount.publicKey.toString()) {
        //console.log('Ignoring another account', keyedAccountInfo);
        return;
      }
      expect(keyedAccountInfo.accountInfo.lamports).toBe(balanceNeeded);
      expect(keyedAccountInfo.accountInfo.owner).toEqual(
        SystemProgram.programId,
      );
      notified = true;
    },
  );

  await connection.requestAirdrop(owner.publicKey, SOL_LAMPORTS);
  try {
    let transaction = SystemProgram.transfer(
      owner.publicKey,
      programAccount.publicKey,
      balanceNeeded,
    );
    await sendAndConfirmTransaction(connection, transaction, owner);
  } catch (err) {
    await connection.removeProgramAccountChangeListener(subscriptionId);
    throw err;
  }

  // Wait for mockCallback to receive a call
  let i = 0;
  while (!notified) {
    //for (;;) {
    if (++i === 30) {
      throw new Error('Program change notification not observed');
    }
    // Sleep for a 1/4 of a slot, notifications only occur after a block is
    // processed
    await sleep((250 * DEFAULT_TICKS_PER_SLOT) / NUM_TICKS_PER_SECOND);
  }

  await connection.removeProgramAccountChangeListener(subscriptionId);
});

test('slot notification', async () => {
  if (mockRpcEnabled) {
    console.log('non-live test skipped');
    return;
  }

  const connection = new Connection(url, 'recent');

  // let notified = false;
  const subscriptionId = connection.onSlotChange(slotInfo => {
    expect(slotInfo.parent).toBeDefined();
    expect(slotInfo.slot).toBeDefined();
    expect(slotInfo.root).toBeDefined();
    expect(slotInfo.slot).toBeGreaterThan(slotInfo.parent);
    expect(slotInfo.slot).toBeGreaterThanOrEqual(slotInfo.root);
    // notified = true;
  });

  //
  // FIXME: enable this check when slotNotification is live
  //
  // // Wait for mockCallback to receive a call
  // let i = 0;
  // while (!notified) {
  //   //for (;;) {
  //   if (++i === 30) {
  //     throw new Error('Slot change notification not observed');
  //   }
  //   // Sleep for a 1/4 of a slot, notifications only occur after a block is
  //   // processed
  //   await sleep((250 * DEFAULT_TICKS_PER_SLOT) / NUM_TICKS_PER_SECOND);
  // }

  await connection.removeSlotChangeListener(subscriptionId);
});
