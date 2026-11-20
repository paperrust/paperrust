// Load the calendar popup only on first open (own chunk, not in the first call).
import type { Component } from 'svelte'

let p: Promise<Component<any>> | undefined
export const loadDatepicker = () => (p ??= import('./Datepicker.svelte').then((m) => m.default))
