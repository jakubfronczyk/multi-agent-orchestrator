import { queryOptions } from "@tanstack/react-query"
import type { AnswerApprovalRequest } from "@/generated/AnswerApprovalRequest"
import type { ApprovalId } from "@/generated/ApprovalId"
import type { ApprovalList } from "@/generated/ApprovalList"
import type { ApprovalStatus } from "@/generated/ApprovalStatus"
import { getJson, postJson, query } from "@/lib/api"

export const approvalsQuery = (status?: ApprovalStatus) =>
  queryOptions({
    queryKey: ["approvals", { status: status ?? null }],
    queryFn: () => getJson<ApprovalList>(`/approvals${query({ status })}`),
  })

/** 409 when the card was already answered, expired or cancelled. */
export const answerApproval = (id: ApprovalId, answer: AnswerApprovalRequest) =>
  postJson<void>(`/approvals/${id}`, answer)
