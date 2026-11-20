from django.db import models


class TaskResult(models.Model):
    task_id = models.CharField(max_length=255, unique=True)
    status = models.CharField(max_length=50, default="PENDING")
    result = models.TextField(null=True)
    date_done = models.DateTimeField(auto_now=True)


class GroupResult(models.Model):
    group_id = models.CharField(max_length=255, unique=True)


class ChordCounter(models.Model):
    group_id = models.CharField(max_length=255, unique=True)
    count = models.PositiveIntegerField()
