export interface CartPayload {
  items?: number[];
}

export function calculateCartSubtotal(
  orderId: number,
  cart: CartPayload | undefined,
): number {
  if (!cart || !cart.items) {
    throw new TypeError(`Cannot compute subtotal for empty cart on order #${orderId}`);
  }
  return cart.items.reduce((sum, price) => sum + price, 0);
}

export function validateDiscountRate(percent: number): number {
  if (percent < 0 || percent > 100) {
    throw new RangeError(`Discount percentage out of bounds: ${percent}`);
  }
  return percent / 100;
}
