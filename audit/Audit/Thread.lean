import Audit.ThreadGen

#assay Sanchoris.Thread.deliver using Sanchoris.Thread.deliver_accepted_iff
  Sanchoris.Thread.deliver_startTurn_iff Sanchoris.Thread.deliver_queue_iff
  Sanchoris.Thread.deliver_reject_iff Sanchoris.Thread.deliver_steer_iff

#assay Sanchoris.Thread.decideStart Sanchoris.Thread.withinPlan Sanchoris.Thread.subscribed using
  Sanchoris.Thread.subscribed_iff Sanchoris.Thread.withinPlan_iff
  Sanchoris.Thread.decideStart_planLimit Sanchoris.Thread.decideStart_start_iff
  Sanchoris.Thread.decideStart_pending_iff
