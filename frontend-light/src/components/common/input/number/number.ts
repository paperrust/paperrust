/** NumberComponent.registerOnChange: integer at step 1 (without exponent), 2 decimal places at step .01 */
export function normalizeNumber(v: any, step: number): any {
  if (step === 1 && v?.toString().indexOf('e') === -1) v = parseInt(v, 10)
  if (step === 0.01) v = parseFloat(v).toFixed(2)
  return v
}
