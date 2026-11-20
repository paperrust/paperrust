from django.db import migrations, models


class Migration(migrations.Migration):
    initial = True
    dependencies = []
    operations = [
        migrations.CreateModel(name="TaskResult", fields=[
            ("id", models.AutoField(primary_key=True, serialize=False)),
            ("task_id", models.CharField(max_length=255, unique=True)),
            ("status", models.CharField(max_length=50, default="PENDING")),
            ("result", models.TextField(null=True)),
            ("date_done", models.DateTimeField(auto_now=True)),
        ]),
        migrations.CreateModel(name="GroupResult", fields=[
            ("id", models.AutoField(primary_key=True, serialize=False)),
            ("group_id", models.CharField(max_length=255, unique=True)),
        ]),
        migrations.CreateModel(name="ChordCounter", fields=[
            ("id", models.AutoField(primary_key=True, serialize=False)),
            ("group_id", models.CharField(max_length=255, unique=True)),
            ("count", models.PositiveIntegerField()),
        ]),
    ]
