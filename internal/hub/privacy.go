package hub

import "github.com/manishtiwari25/pulse/internal/bus"

const heldoutSetting = "evolve.heldout_dir"
const privateValue = bus.PrivateValue

func hiddenSettingValue(value string) string {
	if value == "" {
		return ""
	}
	return privateValue
}

func privateSettingEvent(event additionEvent) additionEvent {
	if bus.PrivateSettings[event.Subject] {
		if value, ok := event.Detail.(string); ok {
			event.Detail = hiddenSettingValue(value)
		} else {
			event.Detail = privateValue
		}
		return event
	}
	if detail, ok := event.Detail.(map[string]any); ok {
		name, _ := detail["name"].(string)
		key, _ := detail["key"].(string)
		if bus.PrivateSettings[name] || bus.PrivateSettings[key] {
			event.Detail = privateValue
		}
	}
	return event
}
