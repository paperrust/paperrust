// AbstractInputComponent.inputId (uuidv4) – only unique, without a library.
let n = 0
export const newInputId = () => (typeof crypto !== 'undefined' && 'randomUUID' in crypto ? crypto.randomUUID() : `input-${++n}`)
