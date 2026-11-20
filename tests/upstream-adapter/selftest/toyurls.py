"""Toy urls/views for the adapter self-test."""

from __future__ import annotations

import threading

from django.http import HttpResponseRedirect, JsonResponse
from django.urls import path
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import AllowAny, IsAuthenticated
from rest_framework.response import Response


@api_view(["GET", "POST"])
@permission_classes([IsAuthenticated])
def whoami(request):
    from django.contrib.auth.models import User

    return Response({
        "user": request.user.username,
        "thread": threading.current_thread().name,
        "q": request.query_params.get("q"),
        "n_users": User.objects.count(),
        "body": request.data if request.method == "POST" else None,
        "next": request.build_absolute_uri("/api/whoami/?page=2"),
    })


@api_view(["GET"])
@permission_classes([AllowAny])
def redirect_view(request):
    return HttpResponseRedirect("/api/whoami/")


def local_view(request):
    return JsonResponse({"thread": threading.current_thread().name})


urlpatterns = [
    path("api/whoami/", whoami),
    path("api/redirect/", redirect_view),
    path("local/", local_view),
]


