import { randomUUID } from "node:crypto"

/** How long unseen or unending background work can hold a finish. */
export const BACKGROUND_WORK_LIMIT = 6 * 3_600_000

/** Order of observations made in one bridge process, independent of its wall clock. */
export type WorkOrder = [origin: string, sequence: number]
const origin = randomUUID()
let sequence = 0
export const workOrder = (): WorkOrder => [origin, ++sequence]
