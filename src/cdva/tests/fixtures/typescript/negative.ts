export interface Money {
  amount: number;
  currency: string;
}

export function add(left: Money, right: Money): Money {
  return { amount: left.amount + right.amount, currency: left.currency };
}

export function format(value: Money): string {
  return `${value.amount.toFixed(2)} ${value.currency}`;
}
