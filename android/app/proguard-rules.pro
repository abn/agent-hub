# Tendril's JNI shim looks its Java peer up by name.
-keep class dev.tendril.** { *; }
-keepclassmembers class dev.tendril.TendrilNative { native <methods>; }
# kotlinx.serialization keeps its generated serializers on its own.

# R8 full mode keeps no constructor that a rule does not name. These classes are
# created by reflection through that constructor: WorkManager's Room database and input
# mergers, and the ML Kit component registrars of the QR scanner.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
-keep class * extends androidx.work.InputMerger { <init>(); }
-keep class * implements com.google.firebase.components.ComponentRegistrar { <init>(); }
