# Tendril's JNI shim looks its Java peer up by name.
-keep class dev.tendril.** { *; }
-keepclassmembers class dev.tendril.TendrilNative { native <methods>; }
# kotlinx.serialization keeps its generated serializers on its own.
