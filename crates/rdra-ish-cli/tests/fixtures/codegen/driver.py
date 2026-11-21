# Driver of tests/codegen_behaviour.rs: runs the Python that `codegen` prints for order.rdra.
from __future__ import annotations

import dataclasses

import domain as d

pending = d.OrderRow(
    id=d.OrderId(1),
    status=d.OrderStatus.PENDING,
    paid_at=None,
    method=d.Method.CARD,
    qty=1,
    note="n",
    customer_id=d.CustomerId(7),
)
order = d.order_from_row(pending)
assert isinstance(order, d.OrderPending), f"a pending row fits pending: {order!r}"
assert order.status == d.OrderStatus.PENDING and order.status == "pending"
assert d.order_to_row(order) == pending, "round trip"

bad = d.order_from_row(dataclasses.replace(pending, paid_at=d.DateTime("t")))
assert isinstance(bad, d.InvalidRow) and bad.column == "paid_at", "pending with a payment time is invalid"

bad_paid = d.order_from_row(dataclasses.replace(pending, status=d.OrderStatus.PAID))
assert isinstance(bad_paid, d.InvalidRow) and bad_paid.column == "paid_at", "paid without a payment time is invalid"

paid = d.run_pay(order, d.DateTime("t"))
assert isinstance(paid, d.PayResult), f"Pay runs from pending: {paid!r}"
assert paid.order.status == d.OrderStatus.PAID
assert paid.order.paid_at == d.DateTime("t")
assert paid.order.qty == 2
assert paid.ev_paid == d.EvPaid() and paid.ev_paid.event == "EvPaid"

again = d.run_pay(paid.order, d.DateTime("u"))
assert isinstance(again, d.NotEnabled) and again.usecase == "Pay", "Pay does not run from paid"

cancelled = d.run_cancel(paid.order)
assert isinstance(cancelled, d.CancelFromPaidResult)
assert cancelled.order.status == d.OrderStatus.CANCELLED

rejected = d.run_cancel(order)
assert isinstance(rejected, d.Rejected) and rejected.condition == "Big", "Big rejects qty 1"

assert d.order_rule_violations(paid.order) == [], "a paid order keeps its rules"
big = dataclasses.replace(paid.order, qty=5)
assert d.order_rule_violations(big) == ["Forbidden_Order_0"], "qty 5 breaks Forbidden_Order_0"

print("ok")
