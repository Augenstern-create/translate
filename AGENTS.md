# Goal 使用策略

- 当用户请求满足任一条件时，自动调用 create_goal，不需要用户说“用 goal”：
  - 任务跨多轮、明确说“继续/直到完成/不要停”；
  - 同时涉及代码修改、测试、回归、报告中的两项以上；
  - 需要连续修复多个失败项；
  - 预计无法在本轮可靠完成。
- 短问答、单文件小改、一次性解释不要创建 goal。
- 创建 goal 时 maxGoalRounds 使用 1000，而不是默认 256，也不是 40。
- goal 保持 active，直到全部验收项完成；不要在中途标记 complete。
- 新会话/新 agent 边界后，如果 goal 持久状态为 active 但 activation 为 disarmed，先 resume，再继续。
- 只有全部无阻塞项完成、测试与报告更新后才 complete。
- 人工评审未执行时保持 PENDING_HUMAN。
