package primitives

type PrimitiveDispatchType string

const (
	PrimitiveDispatchProcessStart      PrimitiveDispatchType = "process.start"
	PrimitiveDispatchProcessWriteInput PrimitiveDispatchType = "process.write_input"
	PrimitiveDispatchProcessCloseInput PrimitiveDispatchType = "process.close_input"
	PrimitiveDispatchProcessSignal     PrimitiveDispatchType = "process.signal"
	PrimitiveDispatchIOCreate          PrimitiveDispatchType = "io.create"
	PrimitiveDispatchIORead            PrimitiveDispatchType = "io.read"
	PrimitiveDispatchRemoteRequest     PrimitiveDispatchType = "remote.request"
	PrimitiveDispatchTimerSchedule     PrimitiveDispatchType = "timer.schedule"
	PrimitiveDispatchCompute           PrimitiveDispatchType = "compute"
)
