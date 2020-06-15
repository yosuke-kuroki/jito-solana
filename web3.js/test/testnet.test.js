// @flow
import {testnetChannelEndpoint} from '../src/util/testnet';

test('invalid', () => {
  expect(() => {
    testnetChannelEndpoint('abc123');
  }).toThrow();
});

test('edge', () => {
  expect(testnetChannelEndpoint('edge')).toEqual(
    'https://edge.testnet.solana.com:8443',
  );

  expect(testnetChannelEndpoint('edge', true)).toEqual(
    'https://edge.testnet.solana.com:8443',
  );

  expect(testnetChannelEndpoint('edge', false)).toEqual(
    'http://edge.testnet.solana.com:8899',
  );
});

test('default', () => {
  testnetChannelEndpoint(); // Should not throw
});
