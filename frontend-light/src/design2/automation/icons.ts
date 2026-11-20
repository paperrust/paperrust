// Icons for the "Automation" area only (loaded lazily, do not burden the first call).
import { addIcons } from '../lib/icons'
import grip from 'bootstrap-icons/icons/grip-vertical.svg?raw'
import play from 'bootstrap-icons/icons/play.svg?raw'
import up from 'bootstrap-icons/icons/arrow-up.svg?raw'
import down from 'bootstrap-icons/icons/arrow-down.svg?raw'
import copy from 'bootstrap-icons/icons/copy.svg?raw'
import lightning from 'bootstrap-icons/icons/lightning.svg?raw'
import split from 'bootstrap-icons/icons/signpost-split.svg?raw'
import right from 'bootstrap-icons/icons/arrow-return-right.svg?raw'

export function registerAutomationIcons() {
  addIcons({
    'grip-vertical': grip,
    play,
    'arrow-up': up,
    'arrow-down': down,
    copy,
    lightning,
    'signpost-split': split,
    'arrow-return-right': right,
  })
}
