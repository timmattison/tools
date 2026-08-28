export function testHelper(value) {
  return value;
}

export function attest(value) {
  return Boolean(value);
}

const witness = testHelper(1);
const proven = attest(witness);
